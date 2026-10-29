//! D-05: PostgreSQL versioned layer checkout. No HTTP endpoint or external funding.
use super::layer_economics::{freeze_term_economics, preflight_term_economics};
use crate::domain::layers::{LayerCheckoutCommand, LayerError, LayerReceipt};
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone)]
pub struct PgLayers {
    pub(super) pool: PgPool,
}
impl PgLayers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Only a book's unique accountable owner can create its selling container.
    pub async fn create_layer(&self, actor: PositiveId, title: &str) -> Result<i64, LayerError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 120 {
            return Err(LayerError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layers(owner_user_id,title) VALUES($1,$2) RETURNING id",
        )
        .bind(actor.get())
        .bind(title)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layer_versions(layer_id,version_no) VALUES($1,1) RETURNING id",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query("UPDATE commerce_layers SET current_version_id=$2 WHERE id=$1")
            .bind(id)
            .bind(version)
            .execute(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(id)
    }

    /// Atomic composition revision: archived versions are never mutated.
    pub async fn add_book(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: PositiveId,
    ) -> Result<i64, LayerError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let row = sqlx::query(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active' FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        let current: i64 = row
            .try_get("current_version_id")
            .map_err(|_| LayerError::Unavailable)?;
        // Serialize Layer composition with B-09 owner transfer and membership
        // mutations on the canonical Book. Exactly one active owner membership
        // is required, and no active collaborator may be attached.
        let owned = sqlx::query_scalar::<_, i64>(
            "SELECT b.id FROM books b
             WHERE b.id=$1 AND b.owner_user_id=$2 AND NOT b.admin_hold
               AND NOT b.is_deleted
               AND EXISTS(
                 SELECT 1 FROM book_members m WHERE m.book_id=b.id
                   AND m.user_id=$2 AND m.is_active AND m.role='owner'
               )
               AND NOT EXISTS(
                 SELECT 1 FROM book_members m WHERE m.book_id=b.id
                   AND m.is_active AND (m.user_id<>$2 OR m.role<>'owner')
               )
             FOR UPDATE OF b",
        )
        .bind(book.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if owned.is_none() {
            return Err(LayerError::Inaccessible);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layer_version_books
             WHERE layer_version_id=$1 AND book_id=$2)",
        )
        .bind(current)
        .bind(book.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if exists {
            return Err(LayerError::Invalid);
        }
        let new_version: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layer_versions(layer_id,version_no)
             SELECT $1, version_no+1 FROM commerce_layer_versions WHERE id=$2 RETURNING id",
        )
        .bind(layer.get())
        .bind(current)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query(
            "INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
             SELECT $1,book_id FROM commerce_layer_version_books WHERE layer_version_id=$2",
        )
        .bind(new_version)
        .bind(current)
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query(
            "INSERT INTO commerce_layer_version_books(layer_version_id,book_id) VALUES($1,$2)",
        )
        .bind(new_version)
        .bind(book.get())
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query("UPDATE commerce_layers SET current_version_id=$2 WHERE id=$1")
            .bind(layer.get())
            .bind(new_version)
            .execute(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(new_version)
    }

    /// Remove from the current offer only: previously purchased composition
    /// versions remain immutable and continue to grant their accrued books.
    pub async fn remove_book(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: PositiveId,
    ) -> Result<i64, LayerError> {
        self.remove_book_internal(actor, layer, book, None).await
    }

    /// Browser removal uses an optimistic current-version precondition.
    /// It is evaluated while holding the Layer row lock, not before it.
    pub async fn remove_book_if_current(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: PositiveId,
        expected_version: PositiveId,
    ) -> Result<i64, LayerError> {
        self.remove_book_internal(actor, layer, book, Some(expected_version))
            .await
    }

    async fn remove_book_internal(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: PositiveId,
        expected_version: Option<PositiveId>,
    ) -> Result<i64, LayerError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let current: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active' FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        if expected_version.is_some_and(|v| v.get() != current) {
            return Err(LayerError::StaleOffer);
        }
        // The same Book lock coordinates removal with transfer/membership
        // checks so a concurrent owner swap cannot observe stale composition.
        let locked: Option<i64> = sqlx::query_scalar("SELECT id FROM books WHERE id=$1 FOR UPDATE")
            .bind(book.get())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        if locked.is_none() {
            return Err(LayerError::Inaccessible);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layer_version_books
             WHERE layer_version_id=$1 AND book_id=$2)",
        )
        .bind(current)
        .bind(book.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if !exists {
            return Err(LayerError::Invalid);
        }
        let next: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layer_versions(layer_id,version_no)
             SELECT $1,version_no+1 FROM commerce_layer_versions WHERE id=$2
             RETURNING id",
        )
        .bind(layer.get())
        .bind(current)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query(
            "INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
             SELECT $1,book_id FROM commerce_layer_version_books
             WHERE layer_version_id=$2 AND book_id<>$3",
        )
        .bind(next)
        .bind(current)
        .bind(book.get())
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query("UPDATE commerce_layers SET current_version_id=$2 WHERE id=$1")
            .bind(layer.get())
            .bind(next)
            .execute(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(next)
    }

    /// A fresh wallet purchase and term, never a mutation of a prior term.
    /// A retry of the same key returns the original historical balance snapshot.
    pub async fn buy(&self, cmd: &LayerCheckoutCommand) -> Result<LayerReceipt, LayerError> {
        cmd.validate()?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let actor = cmd.actor.get();
        let claimed = sqlx::query_scalar::<_, i64>(
            "INSERT INTO commerce_layer_commands
             (user_id,command_key,layer_id,expected_offer_id,expected_version_id,expected_price_minor)
             VALUES($1,$2,$3,$4,$5,$6)
             ON CONFLICT(user_id,command_key) DO NOTHING RETURNING user_id",
        ).bind(actor).bind(cmd.key.as_str()).bind(cmd.layer.get())
            .bind(cmd.offer.get()).bind(cmd.version.get()).bind(cmd.expected_price.get())
            .fetch_optional(&mut *tx).await.map_err(|_| LayerError::Unavailable)?;
        let prior = sqlx::query(
            "SELECT layer_id,expected_offer_id,expected_version_id,
                    expected_price_minor,term_id FROM commerce_layer_commands
             WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
        )
        .bind(actor)
        .bind(cmd.key.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        for (column, expected) in [
            ("layer_id", cmd.layer.get()),
            ("expected_offer_id", cmd.offer.get()),
            ("expected_version_id", cmd.version.get()),
            ("expected_price_minor", cmd.expected_price.get()),
        ] {
            if prior
                .try_get::<i64, _>(column)
                .map_err(|_| LayerError::Unavailable)?
                != expected
            {
                return Err(LayerError::ReplayConflict);
            }
        }
        if claimed.is_none() {
            let term: i64 = prior
                .try_get::<Option<i64>, _>("term_id")
                .map_err(|_| LayerError::Unavailable)?
                .ok_or(LayerError::Unavailable)?;
            let balance: i64 = sqlx::query_scalar(
                "SELECT balance_after_minor FROM reader_wallet_entries
                 WHERE layer_term_id=$1 AND user_id=$2 AND kind='layer_purchase'",
            )
            .bind(term)
            .bind(actor)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
            tx.commit().await.map_err(|_| LayerError::Unavailable)?;
            return Ok(LayerReceipt {
                term_id: term,
                balance_after_minor: balance,
                replayed: true,
            });
        }
        let offer = sqlx::query(
            "SELECT o.term_kind,o.price_minor, l.owner_user_id FROM commerce_layers l
             JOIN commerce_layer_offers o ON o.id=l.current_offer_id AND o.layer_id=l.id
             WHERE l.id=$1 AND l.current_offer_id=$2 AND l.current_version_id=$3
               AND l.status='active' FOR SHARE OF l,o",
        )
        .bind(cmd.layer.get())
        .bind(cmd.offer.get())
        .bind(cmd.version.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::StaleOffer)?;
        let price: i64 = offer
            .try_get("price_minor")
            .map_err(|_| LayerError::Unavailable)?;
        if price != cmd.expected_price.get() {
            return Err(LayerError::StaleOffer);
        }
        if offer
            .try_get::<i64, _>("owner_user_id")
            .map_err(|_| LayerError::Unavailable)?
            == actor
        {
            return Err(LayerError::Inaccessible);
        }
        let book_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM commerce_layer_version_books
             WHERE layer_version_id=$1",
        )
        .bind(cmd.version.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        // S031 v1 refuses any sale whose complete composition cannot be
        // displayed by the bounded, immutable quote page.
        if !(1..=100).contains(&book_count) {
            return Err(LayerError::Inaccessible);
        }
        // The first subscription and every renewal take a Book SHARE lock
        // before issuing a term. Include all historical versions because
        // continuous paid access may still include books removed from live
        // composition. This prevents a grant racing B-09 owner transfer.
        let locked_books: Vec<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b
             WHERE EXISTS (
                 SELECT 1 FROM commerce_layer_versions v
                 JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id
                 WHERE v.layer_id=$1 AND vb.book_id=b.id
             )
             ORDER BY b.id FOR SHARE OF b",
        )
        .bind(cmd.layer.get())
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if locked_books.is_empty() {
            return Err(LayerError::Inaccessible);
        }
        let kind: String = offer
            .try_get("term_kind")
            .map_err(|_| LayerError::Unavailable)?;
        // A stable wallet lock serializes distinct command keys for one buyer.
        // It also makes a renewal chain selection atomic across concurrent buys.
        let available: i64 = sqlx::query_scalar(
            "SELECT balance_minor FROM reader_wallet_accounts
             WHERE user_id=$1 AND currency='RUB' FOR UPDATE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::InsufficientFunds)?;
        if available < price {
            return Err(LayerError::InsufficientFunds);
        }
        let has_lifetime: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layer_terms t
             WHERE t.user_id=$1 AND t.layer_id=$2 AND t.term_kind='lifetime'
             AND NOT EXISTS(
                 SELECT 1 FROM commerce_layer_term_revocations r
                 WHERE r.term_id=t.id
             ))",
        )
        .bind(actor)
        .bind(cmd.layer.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if has_lifetime {
            return Err(LayerError::Inaccessible);
        }
        let required_books =
            preflight_term_economics(&mut tx, actor, cmd.layer.get(), cmd.version.get(), &kind)
                .await?;

        let balance: i64 = sqlx::query_scalar(
            "UPDATE reader_wallet_accounts SET balance_minor=balance_minor-$2
             WHERE user_id=$1 AND currency='RUB' AND balance_minor >= $2
             RETURNING balance_minor",
        )
        .bind(actor)
        .bind(price)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        // A paid renewal starts after the latest non-revoked prepaid term,
        // not at checkout time. Its origin preserves accrued composition
        // versions across the uninterrupted chain; purchases remain separate.
        let term: i64 = sqlx::query_scalar(
            "WITH prior AS (
                 SELECT t.expires_at, t.access_origin_version_id
                 FROM commerce_layer_terms t
                 WHERE t.user_id=$1 AND t.layer_id=$2
                   AND t.term_kind='prepaid_30d' AND t.expires_at>now()
                   AND NOT EXISTS(
                       SELECT 1 FROM commerce_layer_term_revocations r
                       WHERE r.term_id=t.id
                   )
                 ORDER BY t.expires_at DESC,t.id DESC LIMIT 1
             )
             INSERT INTO commerce_layer_terms
             (user_id,layer_id,offer_id,base_version_id,term_kind,amount_minor,
              access_origin_version_id,started_at,expires_at)
             SELECT $1,$2,$3,$4,$5,$6,
                CASE WHEN $5='lifetime' THEN $4
                     ELSE COALESCE((SELECT access_origin_version_id FROM prior),$4)
                END,
                CASE WHEN $5='lifetime' THEN now()
                     ELSE GREATEST(now(),COALESCE((SELECT expires_at FROM prior),now()))
                END,
                CASE WHEN $5='lifetime' THEN NULL
                     ELSE GREATEST(now(),COALESCE((SELECT expires_at FROM prior),now()))
                          + INTERVAL '30 days'
                END
             RETURNING id",
        )
        .bind(actor)
        .bind(cmd.layer.get())
        .bind(cmd.offer.get())
        .bind(cmd.version.get())
        .bind(&kind)
        .bind(price)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        freeze_term_economics(&mut tx, term, required_books).await?;
        let reference = format!("layer-wallet:{actor}:{}", cmd.key.as_str());
        sqlx::query(
            "INSERT INTO reader_wallet_entries
             (user_id,kind,delta_minor,balance_after_minor,reference,layer_term_id)
             VALUES($1,'layer_purchase',$2,$3,$4,$5)",
        )
        .bind(actor)
        .bind(-price)
        .bind(balance)
        .bind(reference)
        .bind(term)
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query(
            "UPDATE commerce_layer_commands SET term_id=$3
             WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(cmd.key.as_str())
        .bind(term)
        .execute(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(LayerReceipt {
            term_id: term,
            balance_after_minor: balance,
            replayed: false,
        })
    }

    /// Active term and version union. This is NOT a direct pinned-V1 purchase.
    /// A term obtains each version published until its expiry, even if a later
    /// version removes a book. Revoking one term preserves rights from others.
    /// Hiding or platform-blocking the storefront never revokes a paid term;
    /// independent Book holds/deletion and term revocations still deny.
    pub async fn can_read_book(
        &self,
        actor: PositiveId,
        book: PositiveId,
    ) -> Result<bool, LayerError> {
        sqlx::query_scalar(
            "SELECT EXISTS (
              SELECT 1 FROM commerce_layer_terms t
              JOIN commerce_layers l ON l.id=t.layer_id
              JOIN commerce_layer_versions origin
                ON origin.id=t.access_origin_version_id
               AND origin.layer_id=t.layer_id
              JOIN commerce_layer_versions v
                ON v.layer_id=t.layer_id AND v.version_no>=origin.version_no
               AND (t.expires_at IS NULL OR v.created_at<t.expires_at)
              JOIN commerce_layer_version_books vb ON vb.layer_version_id=v.id
              JOIN books b ON b.id=vb.book_id
              WHERE t.user_id=$1 AND vb.book_id=$2
                AND t.started_at<=now() AND (t.expires_at IS NULL OR t.expires_at>now())
                AND NOT b.admin_hold AND NOT b.is_deleted
                AND NOT EXISTS(SELECT 1 FROM commerce_layer_term_revocations r WHERE r.term_id=t.id)
            )",
        )
        .bind(actor.get())
        .bind(book.get())
        .fetch_one(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)
    }
}
