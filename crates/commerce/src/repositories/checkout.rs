//! D-02: buyer wallet, immutable purchase and ledger in one PostgreSQL commit.
//! No public funding route or payment provider. Only the owner can request a buy.
use crate::domain::money::{
    CheckoutError, CheckoutRepository, MoneyMinor, PurchaseCommand, PurchaseId, PurchaseReceipt,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

#[derive(Clone)]
pub struct PgCheckout {
    pool: PgPool,
}

fn unavailable(_: sqlx::Error) -> CheckoutError {
    CheckoutError::Unavailable
}

impl PgCheckout {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CheckoutRepository for PgCheckout {
    async fn checkout_once(
        &self,
        command: &PurchaseCommand,
    ) -> Result<PurchaseReceipt, CheckoutError> {
        let actor = command.actor_id.get();
        let chapter = command.edition.chapter_id.get();
        let version = command.edition.version_id.get();
        let price = command.expected_price.get();
        if actor <= 0
            || command.wallet_id.get() != actor
            || chapter <= 0
            || version <= 0
            || price <= 0
        {
            return Err(CheckoutError::Inaccessible);
        }

        let mut tx = self.pool.begin().await.map_err(unavailable)?;

        // The unique (actor, key) claim serializes concurrent retries on every
        // replica. A failed checkout rolls the claim back with the wallet.
        let claimed = sqlx::query_scalar::<_, i64>(
            "INSERT INTO chapter_purchase_commands
              (user_id, command_key, chapter_id, expected_version_id, expected_price_minor)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (user_id, command_key) DO NOTHING RETURNING user_id",
        )
        .bind(actor)
        .bind(command.idempotency_key.as_str())
        .bind(chapter)
        .bind(version)
        .bind(price)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;

        let prior = sqlx::query(
            "SELECT chapter_id, expected_version_id, expected_price_minor, purchase_id
             FROM chapter_purchase_commands
             WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
        )
        .bind(actor)
        .bind(command.idempotency_key.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if prior.try_get::<i64, _>("chapter_id").map_err(unavailable)? != chapter
            || prior
                .try_get::<i64, _>("expected_version_id")
                .map_err(unavailable)?
                != version
            || prior
                .try_get::<i64, _>("expected_price_minor")
                .map_err(unavailable)?
                != price
        {
            return Err(CheckoutError::DuplicateKeyDifferentRequest);
        }

        if claimed.is_none() {
            let id = prior
                .try_get::<Option<i64>, _>("purchase_id")
                .map_err(unavailable)?
                .ok_or(CheckoutError::Unavailable)?;
            // The original balance snapshot is stored in the immutable ledger.
            // Do not return today's wallet balance as a historical receipt.
            let row = sqlx::query(
                "SELECT p.chapter_id, p.version_id, p.amount_minor,
                        e.balance_after_minor
                 FROM chapter_purchase_facts p
                 JOIN reader_wallet_entries e
                   ON e.purchase_id=p.id AND e.user_id=p.user_id
                 WHERE p.id=$1 AND p.user_id=$2 AND e.kind='chapter_purchase'",
            )
            .bind(id)
            .bind(actor)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
            let receipt = receipt(
                id,
                row.try_get("chapter_id").map_err(unavailable)?,
                row.try_get("version_id").map_err(unavailable)?,
                row.try_get("amount_minor").map_err(unavailable)?,
                row.try_get("balance_after_minor").map_err(unavailable)?,
                true,
            )?;
            tx.commit().await.map_err(unavailable)?;
            return Ok(receipt);
        }

        // Hold share locks on the exact published version, current price and
        // moderation state while the money transaction is in progress.
        let offer = sqlx::query(
            "SELECT p.version_id, c.price_amount
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             WHERE c.id=$1 AND c.visibility='public' AND c.access_level='paid'
               AND c.price_amount>0 AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.owner_user_id IS NULL OR b.owner_user_id<>$2)
             FOR SHARE OF c, p, b",
        )
        .bind(chapter)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .ok_or(CheckoutError::Inaccessible)?;
        if offer.try_get::<i64, _>("version_id").map_err(unavailable)? != version
            || offer
                .try_get::<i64, _>("price_amount")
                .map_err(unavailable)?
                != price
        {
            return Err(CheckoutError::StaleOffer);
        }

        // Lock the wallet across DISTINCT purchase keys to prohibit overspend.
        let balance = sqlx::query_scalar::<_, i64>(
            "SELECT balance_minor FROM reader_wallet_accounts
             WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;

        let owned = sqlx::query_scalar::<_, i64>(
            "SELECT purchase_id FROM library_purchased_chapters
             WHERE user_id=$1 AND chapter_id=$2",
        )
        .bind(actor)
        .bind(chapter)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if owned.is_some() {
            return Err(CheckoutError::AlreadyOwned);
        }
        let balance = balance.unwrap_or(0);
        if balance < price {
            return Err(CheckoutError::InsufficientBalance);
        }

        // The conditional debit is defensive even though the wallet is locked.
        let after = sqlx::query_scalar::<_, i64>(
            "UPDATE reader_wallet_accounts SET balance_minor=balance_minor-$2
             WHERE user_id=$1 AND currency='RUB' AND balance_minor >= $2
             RETURNING balance_minor",
        )
        .bind(actor)
        .bind(price)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?
        .ok_or(CheckoutError::InsufficientBalance)?;

        let settlement = format!("wallet:{actor}:{}", command.idempotency_key.as_str());
        // The installed DB trigger creates the pinned LibraryItem and protects
        // it from mutation. A failure there also rolls the wallet debit back.
        let purchase_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO chapter_purchase_facts
              (user_id, chapter_id, version_id, settlement_reference, amount_minor, currency)
             VALUES ($1, $2, $3, $4, $5, 'RUB') RETURNING id",
        )
        .bind(actor)
        .bind(chapter)
        .bind(version)
        .bind(&settlement)
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
                CheckoutError::AlreadyOwned
            } else {
                CheckoutError::Unavailable
            }
        })?;

        sqlx::query(
            "INSERT INTO reader_wallet_entries
              (user_id, kind, delta_minor, balance_after_minor, reference, purchase_id)
             VALUES ($1, 'chapter_purchase', $2, $3, $4, $5)",
        )
        .bind(actor)
        .bind(-price)
        .bind(after)
        .bind(&settlement)
        .bind(purchase_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;

        sqlx::query(
            "UPDATE chapter_purchase_commands SET purchase_id=$3
             WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(command.idempotency_key.as_str())
        .bind(purchase_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;

        tx.commit().await.map_err(unavailable)?;
        receipt(purchase_id, chapter, version, price, after, false)
    }
}

fn receipt(
    purchase_id: i64,
    chapter_id: i64,
    version_id: i64,
    amount: i64,
    balance_after: i64,
    replayed: bool,
) -> Result<PurchaseReceipt, CheckoutError> {
    Ok(PurchaseReceipt {
        purchase_id: PurchaseId::new(purchase_id).map_err(|_| CheckoutError::Unavailable)?,
        pinned: ChapterVersionRef {
            chapter_id: PositiveId::new(chapter_id).map_err(|_| CheckoutError::Unavailable)?,
            version_id: PositiveId::new(version_id).map_err(|_| CheckoutError::Unavailable)?,
        },
        amount: MoneyMinor::new(amount).map_err(|_| CheckoutError::Unavailable)?,
        balance_after: MoneyMinor::new(balance_after).map_err(|_| CheckoutError::Unavailable)?,
        replayed,
    })
}
