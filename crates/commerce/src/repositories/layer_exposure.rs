//! C/D-06: append-only evidence of the FIRST protected Layer content grant.
//! A reader HTML shell, metadata, generic entitlement probe or failed B/G
//! signing must never create evidence. Only C's successful grant HTTP path
//! invokes this gate, after serialization, before returning bearer credentials.
//!
//! This is NOT a complete consumer refund authority: a missing row from
//! pre-migration or pre-cutover history is UNKNOWN, not proof of non-use.
use sqlx::PgPool;
use tor_contracts::paid_delivery::{DeliveryError, PositiveId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerGrantKind {
    Chapter,
    Asset,
}

impl LayerGrantKind {
    fn wire(self) -> &'static str {
        match self {
            Self::Chapter => "chapter",
            Self::Asset => "asset",
        }
    }
}

#[derive(Clone)]
pub struct PgLayerFirstExposure {
    pool: PgPool,
}

// Mirror D's positive, published, active-version membership predicates. A
// blocked Layer storefront does not invalidate an already purchased term.
// Crucially, only terms already started and not expired may be marked:
// a future 30-day renewal must remain separate until its own start time.
const ELIGIBLE_TERMS: &str = r#"
    SELECT t.id
    FROM commerce_layer_terms t
    JOIN chapters c ON c.id=$2
    JOIN books b ON b.id=c.book_id
    JOIN chapter_publications cp ON cp.chapter_id=c.id
    WHERE t.user_id=$1 AND cp.version_id=$3
      AND b.visibility='public' AND c.visibility='public'
      AND c.access_level='paid' AND c.price_amount>0
      AND NOT b.admin_hold AND NOT c.admin_hold
      AND NOT b.is_deleted AND NOT c.is_deleted
      AND (c.reader_released_at IS NULL
           OR c.reader_released_at<=clock_timestamp())
      AND t.started_at<=clock_timestamp()
      AND (t.expires_at IS NULL OR t.expires_at>clock_timestamp())
      AND NOT EXISTS (
          SELECT 1 FROM commerce_layer_term_revocations r
          WHERE r.term_id=t.id
      )
      AND d05_layer_term_has_wallet_evidence(t.id)
      AND EXISTS (
          SELECT 1
          FROM commerce_layer_versions origin
          JOIN commerce_layer_versions v
            ON v.layer_id=t.layer_id
           AND v.version_no>=origin.version_no
           AND (t.expires_at IS NULL OR v.created_at<t.expires_at)
          JOIN commerce_layer_version_books vb
            ON vb.layer_version_id=v.id
          WHERE origin.id=t.access_origin_version_id
            AND origin.layer_id=t.layer_id
            AND vb.book_id=c.book_id
      )
    ORDER BY t.id"#;

impl PgLayerFirstExposure {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Record a successful grant before its signed S3 URL and DEK leave C.
    /// All matching active terms are marked conservatively when more than
    /// one package grants the same chapter. Future financial policy must not
    /// interpret a missing row as evidence the user never consumed content.
    ///
    /// SELECT FOR UPDATE serializes against a future refund/revocation writer
    /// that must lock the SAME term before deciding a reversal. A fresh second
    /// READ COMMITTED statement closes the wait-for-lock revocation race.
    pub async fn record(
        &self,
        actor: PositiveId,
        chapter: PositiveId,
        version: PositiveId,
        kind: LayerGrantKind,
    ) -> Result<(), DeliveryError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        // The default SQLx transaction level is READ COMMITTED. Fresh reads
        // after FOR UPDATE must see revocations committed while waiting.
        let lock_sql = format!("{ELIGIBLE_TERMS} FOR UPDATE OF t");
        let locked: Vec<i64> = sqlx::query_scalar(&lock_sql)
            .bind(actor.get())
            .bind(chapter.get())
            .bind(version.get())
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        if locked.is_empty() {
            return Err(DeliveryError::Denied);
        }
        let still_eligible: Vec<i64> = sqlx::query_scalar(ELIGIBLE_TERMS)
            .bind(actor.get())
            .bind(chapter.get())
            .bind(version.get())
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        if locked != still_eligible {
            return Err(DeliveryError::Denied);
        }
        for term_id in locked {
            // The primary key makes parallel grants exactly-once for each
            // paid term. A subsequent grant never overwrites its first fact.
            sqlx::query(
                "INSERT INTO commerce_layer_term_first_exposures
                    (term_id,chapter_id,version_id,object_kind)
                 VALUES($1,$2,$3,$4) ON CONFLICT(term_id) DO NOTHING",
            )
            .bind(term_id)
            .bind(chapter.get())
            .bind(version.get())
            .bind(kind.wire())
            .execute(&mut *tx)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
            if kind == LayerGrantKind::Chapter {
                // A term's first issued grant may be an image or a DIFFERENT
                // chapter. Record exact chapter text signing separately.
                sqlx::query(
                    "INSERT INTO commerce_layer_term_text_grants
                       (term_id,chapter_id,version_id)
                     VALUES($1,$2,$3) ON CONFLICT(term_id,chapter_id,version_id) DO NOTHING",
                )
                .bind(term_id)
                .bind(chapter.get())
                .bind(version.get())
                .execute(&mut *tx)
                .await
                .map_err(|_| DeliveryError::Unavailable)?;
            }
        }
        tx.commit().await.map_err(|_| DeliveryError::Unavailable)
    }

    /// A client-reported verified chapter render, distinct from issuance.
    /// Only already-granted, currently eligible terms may be confirmed.
    pub async fn confirm_rendered(
        &self,
        actor: PositiveId,
        chapter: PositiveId,
        version: PositiveId,
    ) -> Result<(), DeliveryError> {
        // Shared eligibility SQL includes term window, immutable versions,
        // revocations, moderation hold and D-05 wallet provenance. A recorded
        // credential issuance is an additional necessary precondition.
        let eligible = format!(
            "SELECT t.id FROM commerce_layer_terms t
             JOIN commerce_layer_term_text_grants g ON g.term_id=t.id
               AND g.chapter_id=$2 AND g.version_id=$3
             WHERE t.id IN ({ELIGIBLE_TERMS}) ORDER BY t.id"
        );
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        let locked: Vec<i64> = sqlx::query_scalar(&format!("{eligible} FOR UPDATE OF t"))
            .bind(actor.get())
            .bind(chapter.get())
            .bind(version.get())
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        if locked.is_empty() {
            return Err(DeliveryError::Denied);
        }
        // Fresh post-lock read closes the wait-for-lock revocation race.
        let fresh: Vec<i64> = sqlx::query_scalar(&eligible)
            .bind(actor.get())
            .bind(chapter.get())
            .bind(version.get())
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        if locked != fresh {
            return Err(DeliveryError::Denied);
        }
        for term_id in locked {
            sqlx::query(
                "INSERT INTO commerce_layer_term_render_confirmations
                  (term_id,chapter_id,version_id)
                 VALUES($1,$2,$3) ON CONFLICT(term_id) DO NOTHING",
            )
            .bind(term_id)
            .bind(chapter.get())
            .bind(version.get())
            .execute(&mut *tx)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        }
        tx.commit().await.map_err(|_| DeliveryError::Unavailable)
    }
}
