//! Closed-loop archive invitation purchase. Never exposes a public read grant.
//! Every generation owns its own immutable purchase, not a transferred licence.
use super::{receipt, PgCheckout};
use crate::domain::money::{CheckoutError, PurchaseCommand, PurchaseReceipt};
use sqlx::Row;

impl PgCheckout {
    /// Create a one-version archive offer from an actual paid purchase.
    /// Only the book owner can opt the retired book into archive sales.
    pub async fn create_archive_offer(
        &self,
        sender: i64,
        purchase_id: i64,
        recipient: Option<i64>,
    ) -> Result<String, CheckoutError> {
        if sender <= 0 || purchase_id <= 0 || recipient.is_some_and(|r| r <= 0 || r == sender) {
            return Err(CheckoutError::Inaccessible);
        }
        let row = sqlx::query(
            "INSERT INTO archive_share_offers
               (sender_user_id,sender_purchase_id,chapter_id,version_id,
                quoted_price_minor,parent_offer_token,recipient_user_id)
             SELECT p.user_id,p.id,p.chapter_id,p.version_id,c.price_amount,
                    source.offer_token,$3
             FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l
               ON l.purchase_id=p.id AND l.user_id=p.user_id
              AND l.chapter_id=p.chapter_id AND l.active_version_id=p.version_id
             JOIN chapters c ON c.id=p.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN chapter_versions v ON v.chapter_id=p.chapter_id AND v.id=p.version_id
             JOIN chapter_publications publication ON publication.chapter_id=c.id
             LEFT JOIN archive_purchase_sources source ON source.purchase_id=p.id
             WHERE p.id=$2 AND p.user_id=$1 AND c.access_level='paid'
               AND c.price_amount>0 AND b.archived_at IS NOT NULL
               AND b.archive_invite_sales_enabled AND b.visibility='draft'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND NOT EXISTS (SELECT 1 FROM chapter_purchase_revocations r WHERE r.purchase_id=p.id)
               AND (source.offer_token IS NULL OR
                    EXISTS (SELECT 1 FROM archive_share_offers parent WHERE parent.token=source.offer_token))
             RETURNING token::text",
        )
        .bind(sender).bind(purchase_id).bind(recipient)
        .fetch_optional(&self.pool).await.map_err(|_| CheckoutError::Unavailable)?;
        row.ok_or(CheckoutError::Inaccessible)?
            .try_get("token")
            .map_err(|_| CheckoutError::Unavailable)
    }

    /// Full D wallet debit + historical version + share snapshot in one PG commit.
    /// The caller supplies a trusted 43-character idempotency key through PurchaseCommand.
    pub async fn checkout_archive_once(
        &self,
        offer_token: &str,
        command: &PurchaseCommand,
    ) -> Result<PurchaseReceipt, CheckoutError> {
        let actor = command.actor_id.get();
        let chapter = command.edition.chapter_id.get();
        let version = command.edition.version_id.get();
        let price = command.expected_price.get();
        if actor <= 0 || command.wallet_id.get() != actor || price <= 0 {
            return Err(CheckoutError::Inaccessible);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CheckoutError::Unavailable)?;
        // A committed command must replay its historical receipt even after
        // the original invitation expires, is revoked, or sales are disabled.
        // The command key is global across BOTH purchase pathways.
        let claimed=sqlx::query_scalar::<_,i64>(
            "INSERT INTO chapter_purchase_commands
              (user_id,command_key,chapter_id,expected_version_id,expected_price_minor,archive_offer_token)
             VALUES($1,$2,$3,$4,$5,$6::uuid)
             ON CONFLICT(user_id,command_key) DO NOTHING RETURNING user_id",
        ).bind(actor).bind(command.idempotency_key.as_str()).bind(chapter).bind(version)
        .bind(price).bind(offer_token).fetch_optional(&mut *tx).await.map_err(|_| CheckoutError::Unavailable)?;
        let prior = sqlx::query(
            "SELECT chapter_id,expected_version_id,expected_price_minor,purchase_id,
                    archive_offer_token::text AS archive_offer_token
             FROM chapter_purchase_commands WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
        )
        .bind(actor)
        .bind(command.idempotency_key.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CheckoutError::Unavailable)?;
        if prior
            .try_get::<i64, _>("chapter_id")
            .map_err(|_| CheckoutError::Unavailable)?
            != chapter
            || prior
                .try_get::<i64, _>("expected_version_id")
                .map_err(|_| CheckoutError::Unavailable)?
                != version
            || prior
                .try_get::<i64, _>("expected_price_minor")
                .map_err(|_| CheckoutError::Unavailable)?
                != price
            || prior
                .try_get::<Option<String>, _>("archive_offer_token")
                .map_err(|_| CheckoutError::Unavailable)?
                .as_deref()
                != Some(offer_token)
        {
            return Err(CheckoutError::DuplicateKeyDifferentRequest);
        }
        if claimed.is_none() {
            let id: i64 = prior
                .try_get::<Option<i64>, _>("purchase_id")
                .map_err(|_| CheckoutError::Unavailable)?
                .ok_or(CheckoutError::Unavailable)?;
            let row = sqlx::query(
                "SELECT p.amount_minor,e.balance_after_minor
                 FROM chapter_purchase_facts p JOIN reader_wallet_entries e
                   ON e.purchase_id=p.id AND e.user_id=p.user_id AND e.kind='chapter_purchase'
                 JOIN archive_purchase_sources source ON source.purchase_id=p.id
                 WHERE p.id=$1 AND p.user_id=$2 AND source.offer_token=$3::uuid",
            )
            .bind(id)
            .bind(actor)
            .bind(offer_token)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| CheckoutError::Unavailable)?;
            let answer = receipt(
                id,
                chapter,
                version,
                row.try_get("amount_minor")
                    .map_err(|_| CheckoutError::Unavailable)?,
                row.try_get("balance_after_minor")
                    .map_err(|_| CheckoutError::Unavailable)?,
                true,
            )?;
            tx.commit().await.map_err(|_| CheckoutError::Unavailable)?;
            return Ok(answer);
        }
        // Only NEW purchases require the invitation to remain active.
        let offer=sqlx::query(
            "SELECT o.sender_user_id,o.sender_purchase_id,o.chapter_id,o.version_id,
                    o.quoted_price_minor,b.id AS book_id,st.current_revision AS share_revision
             FROM archive_share_offers o
             JOIN chapter_purchase_facts src ON src.id=o.sender_purchase_id
                AND src.user_id=o.sender_user_id AND src.chapter_id=o.chapter_id
                AND src.version_id=o.version_id
             JOIN library_purchased_chapters l ON l.purchase_id=src.id
                AND l.user_id=src.user_id AND l.chapter_id=src.chapter_id
                AND l.active_version_id=src.version_id
             JOIN chapters c ON c.id=o.chapter_id
             JOIN books b ON b.id=c.book_id
             JOIN revenue_share_states st ON st.book_id=b.id
             JOIN chapter_versions v ON v.chapter_id=o.chapter_id AND v.id=o.version_id
             JOIN chapter_publications publication ON publication.chapter_id=c.id
             WHERE o.token=$1::uuid AND o.revoked_at IS NULL AND o.expires_at>now()
               AND (o.recipient_user_id IS NULL OR o.recipient_user_id=$2)
               AND o.sender_user_id<>$2
               AND o.chapter_id=$3 AND o.version_id=$4 AND o.quoted_price_minor=$5
               AND b.archived_at IS NOT NULL AND b.archive_invite_sales_enabled
               AND b.visibility='draft' AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.owner_user_id IS NULL OR b.owner_user_id<>$2)
               AND c.access_level='paid' AND c.price_amount=$5
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND NOT EXISTS (SELECT 1 FROM chapter_purchase_revocations r WHERE r.purchase_id=src.id)
             FOR SHARE OF o,b,c",
        ).bind(offer_token).bind(actor).bind(chapter).bind(version).bind(price)
        .fetch_optional(&mut *tx).await.map_err(|_| CheckoutError::Unavailable)?
        .ok_or(CheckoutError::Inaccessible)?;
        let book: i64 = offer
            .try_get("book_id")
            .map_err(|_| CheckoutError::Unavailable)?;
        let revision: i64 = offer
            .try_get("share_revision")
            .map_err(|_| CheckoutError::Unavailable)?;
        let total: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(share_percent),0)::BIGINT FROM revenue_share_assignments
             WHERE book_id=$1 AND revision=$2",
        )
        .bind(book)
        .bind(revision)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| CheckoutError::Unavailable)?;
        if total != 100 {
            return Err(CheckoutError::Unavailable);
        }
        let balance=sqlx::query_scalar::<_,i64>(
            "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
        ).bind(actor).fetch_optional(&mut *tx).await.map_err(|_| CheckoutError::Unavailable)?.unwrap_or(0);
        if sqlx::query_scalar::<_, i64>(
            "SELECT purchase_id FROM library_purchased_chapters WHERE user_id=$1 AND chapter_id=$2",
        )
        .bind(actor)
        .bind(chapter)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CheckoutError::Unavailable)?
        .is_some()
        {
            return Err(CheckoutError::AlreadyOwned);
        }
        if balance < price {
            return Err(CheckoutError::InsufficientBalance);
        }
        let after = sqlx::query_scalar::<_, i64>(
            "UPDATE reader_wallet_accounts SET balance_minor=balance_minor-$2
             WHERE user_id=$1 AND currency='RUB' AND balance_minor>=$2 RETURNING balance_minor",
        )
        .bind(actor)
        .bind(price)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CheckoutError::Unavailable)?
        .ok_or(CheckoutError::InsufficientBalance)?;
        let settlement = format!("wallet:{actor}:{}", command.idempotency_key.as_str());
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO chapter_purchase_facts
               (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
             VALUES($1,$2,$3,$4,$5,'RUB') RETURNING id",
        )
        .bind(actor)
        .bind(chapter)
        .bind(version)
        .bind(&settlement)
        .bind(price)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23505") {
                CheckoutError::AlreadyOwned
            } else {
                CheckoutError::Unavailable
            }
        })?;
        sqlx::query(
            "INSERT INTO chapter_purchase_share_snapshots
                (purchase_id,book_id,share_revision,gross_amount_minor) VALUES($1,$2,$3,$4)",
        )
        .bind(id)
        .bind(book)
        .bind(revision)
        .bind(price)
        .execute(&mut *tx)
        .await
        .map_err(|_| CheckoutError::Unavailable)?;
        sqlx::query(
            "INSERT INTO chapter_purchase_share_lines(purchase_id,user_id,share_percent)
             SELECT $1,user_id,share_percent FROM revenue_share_assignments WHERE book_id=$2 AND revision=$3",
        ).bind(id).bind(book).bind(revision).execute(&mut *tx).await.map_err(|_| CheckoutError::Unavailable)?;
        sqlx::query(
            "INSERT INTO reader_wallet_entries(user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
             VALUES($1,'chapter_purchase',$2,$3,$4,$5)",
        ).bind(actor).bind(-price).bind(after).bind(&settlement).bind(id)
        .execute(&mut *tx).await.map_err(|_| CheckoutError::Unavailable)?;
        sqlx::query(
            "INSERT INTO archive_purchase_sources(purchase_id,offer_token) VALUES($1,$2::uuid)",
        )
        .bind(id)
        .bind(offer_token)
        .execute(&mut *tx)
        .await
        .map_err(|_| CheckoutError::Unavailable)?;
        sqlx::query(
            "UPDATE chapter_purchase_commands SET purchase_id=$3 WHERE user_id=$1 AND command_key=$2",
        ).bind(actor).bind(command.idempotency_key.as_str()).bind(id)
        .execute(&mut *tx).await.map_err(|_| CheckoutError::Unavailable)?;
        tx.commit().await.map_err(|_| CheckoutError::Unavailable)?;
        receipt(id, chapter, version, price, after, false)
    }
}
