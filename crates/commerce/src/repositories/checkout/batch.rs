//! D/S035: atomic multi-chapter purchase on the canonical wallet ledger.
//! A single PostgreSQL commit owns all grants, debits, provenance and replay.
use super::{receipt, PgCheckout};
use crate::domain::{
    batch_checkout::{BatchCheckoutError, BatchPurchaseCommand, BatchPurchaseReceipt},
    money::{CheckoutError, PurchaseReceipt},
};
use sqlx::Row;

#[derive(Clone, Debug)]
pub struct SelectableBatchOffer {
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub title: String,
    pub version_id: i64,
    pub price_minor: i64,
}

impl PgCheckout {
    /// Private, user-scoped offer metadata for native browser selection.
    /// Never returns bodies or editions from hidden/held books; maximum 101 rows.
    pub async fn selectable_batch_offers(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Vec<SelectableBatchOffer>, CheckoutError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT c.id,c.chapter_no,c.title,c.price_amount,p.version_id
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
             WHERE b.id=$1 AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='public' AND c.access_level='paid' AND c.price_amount>0
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND (b.owner_user_id IS NULL OR b.owner_user_id<>$2)
               AND NOT EXISTS (
                   SELECT 1 FROM chapter_purchase_facts owned
                   WHERE owned.user_id=$2 AND owned.chapter_id=c.id
               )
             ORDER BY c.chapter_no,c.id LIMIT 101",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CheckoutError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(SelectableBatchOffer {
                    chapter_id: row.try_get("id").map_err(|_| CheckoutError::Unavailable)?,
                    chapter_no: row
                        .try_get("chapter_no")
                        .map_err(|_| CheckoutError::Unavailable)?,
                    title: row
                        .try_get("title")
                        .map_err(|_| CheckoutError::Unavailable)?,
                    version_id: row
                        .try_get("version_id")
                        .map_err(|_| CheckoutError::Unavailable)?,
                    price_minor: row
                        .try_get("price_amount")
                        .map_err(|_| CheckoutError::Unavailable)?,
                })
            })
            .collect()
    }
}

fn unavailable(_: sqlx::Error) -> BatchCheckoutError {
    CheckoutError::Unavailable.into()
}

impl PgCheckout {
    /// Buy 2..20 unowned paid chapters of one book, or buy none.
    /// The input quote is sorted and SHA-256-bound to its user-scoped key.
    pub async fn checkout_batch_once(
        &self,
        command: &BatchPurchaseCommand,
    ) -> Result<BatchPurchaseReceipt, BatchCheckoutError> {
        let actor = command.actor().get();
        if command.wallet().get() != actor {
            return Err(BatchCheckoutError::InvalidBatch);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let count =
            i16::try_from(command.offers().len()).map_err(|_| BatchCheckoutError::InvalidBatch)?;

        // Unique (actor,key) is the one global claim. Concurrent retries
        // wait for this transaction, never repeat the wallet debit.
        let claimed = sqlx::query_scalar::<_, i64>(
            "INSERT INTO chapter_batch_commands
               (user_id,command_key,request_sha256,item_count)
             VALUES($1,$2,$3,$4)
             ON CONFLICT (user_id,command_key) DO NOTHING
             RETURNING user_id",
        )
        .bind(actor)
        .bind(command.key().as_str())
        .bind(command.request_sha256().as_slice())
        .bind(count)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let prior = sqlx::query(
            "SELECT request_sha256,item_count
             FROM chapter_batch_commands
             WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
        )
        .bind(actor)
        .bind(command.key().as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        let original_hash: Vec<u8> = prior.try_get("request_sha256").map_err(unavailable)?;
        let original_count: i16 = prior.try_get("item_count").map_err(unavailable)?;
        if original_hash.as_slice() != command.request_sha256() || original_count != count {
            return Err(CheckoutError::DuplicateKeyDifferentRequest.into());
        }

        if claimed.is_none() {
            // Never return the *current* wallet balance as a historic receipt.
            // Every paid line carries its own immutable journal snapshot.
            let rows = sqlx::query(
                "SELECT p.id,p.chapter_id,p.version_id,p.amount_minor,
                        e.balance_after_minor
                 FROM chapter_batch_items i
                 JOIN chapter_purchase_facts p
                   ON p.id=i.purchase_id AND p.user_id=i.user_id
                 JOIN reader_wallet_entries e
                   ON e.purchase_id=p.id AND e.user_id=p.user_id
                  AND e.kind='chapter_purchase'
                 WHERE i.user_id=$1 AND i.command_key=$2
                 ORDER BY i.position",
            )
            .bind(actor)
            .bind(command.key().as_str())
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
            if rows.len() != command.offers().len() {
                return Err(CheckoutError::Unavailable.into());
            }
            let mut purchases = Vec::with_capacity(rows.len());
            for (offer, row) in command.offers().iter().zip(&rows) {
                let chapter: i64 = row.try_get("chapter_id").map_err(unavailable)?;
                let version: i64 = row.try_get("version_id").map_err(unavailable)?;
                let amount: i64 = row.try_get("amount_minor").map_err(unavailable)?;
                if chapter != offer.edition.chapter_id.get()
                    || version != offer.edition.version_id.get()
                    || amount != offer.expected_price.get()
                {
                    return Err(CheckoutError::Unavailable.into());
                }
                let id: i64 = row.try_get("id").map_err(unavailable)?;
                let balance: i64 = row.try_get("balance_after_minor").map_err(unavailable)?;
                purchases.push(
                    receipt(id, chapter, version, amount, balance, true)
                        .map_err(BatchCheckoutError::from)?,
                );
            }
            let balance_after = purchases
                .last()
                .map(|item| item.balance_after)
                .ok_or(BatchCheckoutError::InvalidBatch)?;
            tx.commit().await.map_err(unavailable)?;
            return Ok(BatchPurchaseReceipt {
                purchases,
                total: command.total(),
                balance_after,
                replayed: true,
            });
        }

        // Lock all eligible offers BEFORE the wallet, just as D-02 does.
        // One Book only; this avoids lock inversions with another batch and
        // prevents an unrelated Book's terms being sold accidentally.
        let mut books_and_revisions = Vec::with_capacity(command.offers().len());
        let mut one_book = None;
        for offer in command.offers() {
            let row = sqlx::query(
                "SELECT p.version_id,c.price_amount,b.id AS book_id,
                        st.current_revision AS share_revision
                 FROM chapters c
                 JOIN books b ON b.id=c.book_id
                 JOIN revenue_share_states st ON st.book_id=b.id
                 JOIN chapter_publications p ON p.chapter_id=c.id
                 JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
                 WHERE c.id=$1 AND c.visibility='public' AND c.access_level='paid'
                   AND c.price_amount>0 AND NOT c.admin_hold AND NOT c.is_deleted
                   AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
                   AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
                   AND (b.owner_user_id IS NULL OR b.owner_user_id<>$2)
                 FOR SHARE OF c,p,b",
            )
            .bind(offer.edition.chapter_id.get())
            .bind(actor)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .ok_or(CheckoutError::Inaccessible)?;
            let version: i64 = row.try_get("version_id").map_err(unavailable)?;
            let price: i64 = row.try_get("price_amount").map_err(unavailable)?;
            if version != offer.edition.version_id.get() || price != offer.expected_price.get() {
                return Err(CheckoutError::StaleOffer.into());
            }
            let book: i64 = row.try_get("book_id").map_err(unavailable)?;
            let revision: i64 = row.try_get("share_revision").map_err(unavailable)?;
            if one_book.is_some_and(|first| first != book) {
                return Err(BatchCheckoutError::MixedBooks);
            }
            one_book = Some(book);
            let sum: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(share_percent),0)::BIGINT
                 FROM revenue_share_assignments
                 WHERE book_id=$1 AND revision=$2",
            )
            .bind(book)
            .bind(revision)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
            if sum != 100 {
                return Err(CheckoutError::Unavailable.into());
            }
            let terms = if self.creator_posting_enabled {
                Some(
                    super::revenue::load_terms(
                        &mut tx,
                        book,
                        revision,
                        offer.expected_price.get(),
                    )
                    .await
                    .map_err(BatchCheckoutError::from)?,
                )
            } else {
                None
            };
            books_and_revisions.push((book, revision, terms));
        }

        let balance = sqlx::query_scalar::<_, i64>(
            "SELECT balance_minor FROM reader_wallet_accounts
             WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .unwrap_or(0);
        for offer in command.offers() {
            let already_owned = sqlx::query_scalar::<_, i64>(
                "SELECT purchase_id FROM library_purchased_chapters
                 WHERE user_id=$1 AND chapter_id=$2",
            )
            .bind(actor)
            .bind(offer.edition.chapter_id.get())
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            if already_owned.is_some() {
                return Err(CheckoutError::AlreadyOwned.into());
            }
        }
        if balance < command.total().get() {
            return Err(CheckoutError::InsufficientBalance.into());
        }

        let mut purchases: Vec<PurchaseReceipt> = Vec::with_capacity(command.offers().len());
        for (index, (offer, (book, revision, terms))) in
            command.offers().iter().zip(books_and_revisions).enumerate()
        {
            let price = offer.expected_price.get();
            let after = sqlx::query_scalar::<_, i64>(
                "UPDATE reader_wallet_accounts
                 SET balance_minor=balance_minor-$2
                 WHERE user_id=$1 AND currency='RUB' AND balance_minor>=$2
                 RETURNING balance_minor",
            )
            .bind(actor)
            .bind(price)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?
            .ok_or(CheckoutError::InsufficientBalance)?;
            let position =
                i16::try_from(index + 1).map_err(|_| BatchCheckoutError::InvalidBatch)?;
            let journal_reference =
                format!("batch-wallet:{actor}:{}:{position}", command.key().as_str());
            let chapter = offer.edition.chapter_id.get();
            let version = offer.edition.version_id.get();
            let id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO chapter_purchase_facts
                   (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
                 VALUES($1,$2,$3,$4,$5,'RUB') RETURNING id",
            )
            .bind(actor)
            .bind(chapter)
            .bind(version)
            .bind(&journal_reference)
            .bind(price)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| {
                if error
                    .as_database_error()
                    .and_then(|db| db.code())
                    .as_deref()
                    == Some("23505")
                {
                    BatchCheckoutError::Checkout(CheckoutError::AlreadyOwned)
                } else {
                    unavailable(error)
                }
            })?;
            sqlx::query(
                "INSERT INTO chapter_purchase_share_snapshots
                   (purchase_id,book_id,share_revision,gross_amount_minor)
                 VALUES($1,$2,$3,$4)",
            )
            .bind(id)
            .bind(book)
            .bind(revision)
            .bind(price)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            sqlx::query(
                "INSERT INTO chapter_purchase_share_lines
                   (purchase_id,user_id,share_percent)
                 SELECT $1,user_id,share_percent
                 FROM revenue_share_assignments
                 WHERE book_id=$2 AND revision=$3",
            )
            .bind(id)
            .bind(book)
            .bind(revision)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            sqlx::query(
                "INSERT INTO reader_wallet_entries
                   (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
                 VALUES($1,'chapter_purchase',$2,$3,$4,$5)",
            )
            .bind(actor)
            .bind(-price)
            .bind(after)
            .bind(&journal_reference)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            if let Some(terms) = &terms {
                super::revenue::post_purchase(&mut tx, id, book, revision, terms)
                    .await
                    .map_err(BatchCheckoutError::from)?;
            }
            sqlx::query(
                "INSERT INTO chapter_batch_items
                   (user_id,command_key,position,purchase_id)
                 VALUES($1,$2,$3,$4)",
            )
            .bind(actor)
            .bind(command.key().as_str())
            .bind(position)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            purchases.push(
                receipt(id, chapter, version, price, after, false)
                    .map_err(BatchCheckoutError::from)?,
            );
        }
        tx.commit().await.map_err(unavailable)?;
        let balance_after = purchases
            .last()
            .map(|item| item.balance_after)
            .ok_or(BatchCheckoutError::InvalidBatch)?;
        Ok(BatchPurchaseReceipt {
            purchases,
            total: command.total(),
            balance_after,
            replayed: false,
        })
    }
}
