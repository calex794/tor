//! C-06: append-only first protected-grant issuance for direct purchased V1.
//! No proof of successful download/decryption; historical missing rows remain
//! UNKNOWN and never establish a buyer's eligibility for a refund.
use sqlx::PgPool;
use tor_contracts::paid_delivery::{DeliveryError, PositiveId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectGrantKind {
    Chapter,
    Asset,
}
impl DirectGrantKind {
    fn wire(self) -> &'static str {
        match self {
            Self::Chapter => "chapter",
            Self::Asset => "asset",
        }
    }
}

#[derive(Clone)]
pub struct PgDirectFirstExposure {
    pool: PgPool,
}

impl PgDirectFirstExposure {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// After C assembles the encrypted grant, record the still-owned
    /// exact pinned purchase before returning its signed bearer URL.
    /// FOR UPDATE on the purchase conflicts with the revocation FK's
    /// key-share; a fresh READ COMMITTED statement closes the lock-wait race.
    pub async fn record(
        &self,
        actor: PositiveId,
        chapter: PositiveId,
        version: PositiveId,
        purchase: Option<PositiveId>,
        kind: DirectGrantKind,
    ) -> Result<(), DeliveryError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        let owned: Option<i64> = sqlx::query_scalar(
            "SELECT p.id
             FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l
               ON l.purchase_id=p.id AND l.user_id=p.user_id
              AND l.chapter_id=p.chapter_id AND l.active_version_id=p.version_id
             WHERE p.user_id=$1 AND p.chapter_id=$2 AND p.version_id=$3
               AND ($4::BIGINT IS NULL OR p.id=$4)
               AND tor_purchase_version_readable($1,p.id,p.chapter_id,p.version_id)
             FOR UPDATE OF p",
        )
        .bind(actor.get())
        .bind(chapter.get())
        .bind(version.get())
        .bind(purchase.map(PositiveId::get))
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        let purchase_id = owned.ok_or(DeliveryError::Denied)?;
        let still_readable: bool =
            sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
                .bind(actor.get())
                .bind(purchase_id)
                .bind(chapter.get())
                .bind(version.get())
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| DeliveryError::Unavailable)?;
        if !still_readable {
            return Err(DeliveryError::Denied);
        }
        sqlx::query(
            "INSERT INTO chapter_purchase_first_exposures
               (purchase_id,user_id,chapter_id,version_id,object_kind)
             VALUES($1,$2,$3,$4,$5) ON CONFLICT(purchase_id) DO NOTHING",
        )
        .bind(purchase_id)
        .bind(actor.get())
        .bind(chapter.get())
        .bind(version.get())
        .bind(kind.wire())
        .execute(&mut *tx)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        tx.commit().await.map_err(|_| DeliveryError::Unavailable)
    }

    /// A browser reports first successful WebCrypto validation and DOM render.
    /// The server cannot prove human attention; no ACK cannot prove non-use.
    /// Reuse the exact paid purchase authority and lock used by the grant gate.
    pub async fn confirm_rendered(
        &self,
        actor: PositiveId,
        chapter: PositiveId,
        version: PositiveId,
        purchase: PositiveId,
    ) -> Result<(), DeliveryError> {
        let mut tx = self.pool.begin().await.map_err(|_| DeliveryError::Unavailable)?;
        let owned: Option<i64> = sqlx::query_scalar(
            "SELECT p.id
             FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l
               ON l.purchase_id=p.id AND l.user_id=p.user_id
              AND l.chapter_id=p.chapter_id AND l.active_version_id=p.version_id
             JOIN chapter_purchase_first_exposures g ON g.purchase_id=p.id
             WHERE p.id=$4 AND p.user_id=$1
               AND p.chapter_id=$2 AND p.version_id=$3
               AND tor_purchase_version_readable($1,p.id,p.chapter_id,p.version_id)
             FOR UPDATE OF p",
        )
        .bind(actor.get())
        .bind(chapter.get())
        .bind(version.get())
        .bind(purchase.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        if owned.is_none() {
            return Err(DeliveryError::Denied);
        }
        // A fresh READ COMMITTED statement notices any revocation committed
        // while the transaction waited for the purchase lock.
        let still_readable: bool =
            sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
                .bind(actor.get())
                .bind(purchase.get())
                .bind(chapter.get())
                .bind(version.get())
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| DeliveryError::Unavailable)?;
        if !still_readable {
            return Err(DeliveryError::Denied);
        }
        sqlx::query(
            "INSERT INTO chapter_purchase_render_confirmations
             (purchase_id,user_id,chapter_id,version_id)
             VALUES($1,$2,$3,$4) ON CONFLICT(purchase_id) DO NOTHING",
        )
        .bind(purchase.get())
        .bind(actor.get())
        .bind(chapter.get())
        .bind(version.get())
        .execute(&mut *tx)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        tx.commit().await.map_err(|_| DeliveryError::Unavailable)
    }
}
