//! D-05 / S034: resolve an active paid Layer to the current published
//! chapter edition. Book-level Layer membership is never an S3 grant.
//! The publisher (B) and private object signer (G) independently verify
//! the exact returned version before any encrypted bytes are released.
use super::entitlements::PgDirectEntitlements;
use async_trait::async_trait;
use sqlx::PgPool;
use tor_contracts::paid_delivery::{
    Actor, ChapterVersionRef, DeliveryError, DenialReason, EntitlementDecision, EntitlementPort,
    PositiveId,
};

#[derive(Clone)]
pub struct PgLayerChapterEntitlements {
    pool: PgPool,
}

impl PgLayerChapterEntitlements {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl EntitlementPort for PgLayerChapterEntitlements {
    async fn resolve_entitlement(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
        purchase_id: Option<PositiveId>,
    ) -> Result<EntitlementDecision, DeliveryError> {
        // A direct purchase identity is never a Layer credential.
        if purchase_id.is_some() {
            return Ok(EntitlementDecision::Denied {
                reason: DenialReason::NotPurchased,
            });
        }

        // Both the version and the book entitlement are read from PostgreSQL.
        // A Layer is a book-level package, but the concrete chapter must be
        // public, paid, released and published before it may be delivered.
        // Hidden/blocked Layer storefronts must not revoke historical terms.
        let record: Option<(Option<i64>, bool, bool)> = sqlx::query_as(
            "SELECT cp.version_id,
                    (b.visibility='public' AND c.visibility='public'
                     AND c.access_level='paid' AND c.price_amount>0
                     AND NOT b.admin_hold AND NOT b.is_deleted
                     AND NOT c.admin_hold AND NOT c.is_deleted
                     AND (c.reader_released_at IS NULL
                          OR c.reader_released_at<=now())) AS readable,
                    EXISTS (
                        SELECT 1 FROM commerce_layer_terms t
                        JOIN commerce_layer_versions origin
                          ON origin.id=t.access_origin_version_id
                         AND origin.layer_id=t.layer_id
                        JOIN commerce_layer_versions v
                          ON v.layer_id=t.layer_id
                         AND v.version_no>=origin.version_no
                         AND (t.expires_at IS NULL OR v.created_at<t.expires_at)
                        JOIN commerce_layer_version_books vb
                          ON vb.layer_version_id=v.id
                        WHERE t.user_id=$1 AND vb.book_id=c.book_id
                          AND t.started_at<=now()
                          AND (t.expires_at IS NULL OR t.expires_at>now())
                          AND NOT EXISTS (
                              SELECT 1 FROM commerce_layer_term_revocations r
                              WHERE r.term_id=t.id
                          )
                    ) AS entitled
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             LEFT JOIN chapter_publications cp ON cp.chapter_id=c.id
             WHERE c.id=$2",
        )
        .bind(actor.user_id.get())
        .bind(chapter_id.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;

        let Some((version, readable, entitled)) = record else {
            return Ok(EntitlementDecision::Denied {
                reason: DenialReason::Hidden,
            });
        };
        if !readable || version.is_none() {
            return Ok(EntitlementDecision::Denied {
                reason: DenialReason::Hidden,
            });
        }
        if !entitled {
            return Ok(EntitlementDecision::Denied {
                reason: DenialReason::NotPurchased,
            });
        }
        let version_id = version
            .and_then(|id| PositiveId::new(id).ok())
            .ok_or(DeliveryError::Unavailable)?;
        Ok(EntitlementDecision::Allowed {
            pinned: ChapterVersionRef {
                chapter_id,
                version_id,
            },
        })
    }
}

/// D-owned combination for the existing C/B/G paid-grant port. Explicit
/// purchase IDs always select the original V1; without one, direct purchases
/// take priority and Layer may grant the current published edition.
/// Hard denials and database inconsistencies never fall through to Layer.
#[derive(Clone)]
pub struct PgReaderEntitlements {
    direct: PgDirectEntitlements,
    layer: PgLayerChapterEntitlements,
}

impl PgReaderEntitlements {
    pub fn new(pool: PgPool) -> Self {
        Self {
            direct: PgDirectEntitlements::new(pool.clone()),
            layer: PgLayerChapterEntitlements::new(pool),
        }
    }
}

#[async_trait]
impl EntitlementPort for PgReaderEntitlements {
    async fn resolve_entitlement(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
        purchase_id: Option<PositiveId>,
    ) -> Result<EntitlementDecision, DeliveryError> {
        let direct = self
            .direct
            .resolve_entitlement(actor, chapter_id, purchase_id)
            .await?;
        if purchase_id.is_some() {
            return Ok(direct);
        }
        match direct {
            EntitlementDecision::Allowed { .. }
            | EntitlementDecision::Denied {
                reason: DenialReason::Locked | DenialReason::Hidden | DenialReason::Unknown,
            } => Ok(direct),
            EntitlementDecision::Denied {
                reason: DenialReason::NotPurchased | DenialReason::Refunded,
            } => {
                let layer = self
                    .layer
                    .resolve_entitlement(actor, chapter_id, None)
                    .await?;
                match layer {
                    EntitlementDecision::Allowed { .. } => Ok(layer),
                    EntitlementDecision::Denied { .. } => Ok(direct),
                }
            }
        }
    }
}
