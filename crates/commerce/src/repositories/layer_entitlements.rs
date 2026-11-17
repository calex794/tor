//! D-05 / BR-060: resolve an active paid Layer to the published edition.
//! Ordinary-deleted Books stay privately readable for previously acquired
//! terms, but may not join the rights of buyers arriving after deletion.
//! Book-level Layer membership is never an S3 grant.
//! The publisher (B) and private object signer (G) independently verify
//! the exact returned version before any encrypted bytes are released.
pub mod toc;
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
        // A hidden/blocked Layer *storefront* does not revoke historical terms.
        // A site-moderation Book block or revoked 18+/21+ viewer gate does.
        // An ordinary Book deletion is different: historical Layer terms
        // remain valid until expiry, while new buyers never gain that Book.
        // No direct purchase fact is inferred from Layer membership.
        let record: Option<(Option<i64>, bool, bool, bool)> = sqlx::query_as(
            "SELECT cp.version_id,
                    ((b.visibility='public' OR b.lifecycle_status='pending_deletion' OR
                       (b.lifecycle_status='ordinary_deleted'
                        AND b.ordinary_deleted_at IS NOT NULL))
                     AND c.visibility='public'
                     AND c.access_level='paid' AND c.price_amount>0
                     AND viewer.account_state='active'
                     AND b.visibility_status='active'
                     AND COALESCE((
                         b.access_level='basic'
                         OR (b.access_level='adult' AND viewer.adult_enabled)
                         OR (b.access_level='pro_adult' AND EXISTS (
                             SELECT 1 FROM user_level_grants AS age_grant
                             WHERE age_grant.user_id=viewer.id
                               AND age_grant.level='pro_adult'
                               AND age_grant.is_active
                         ))
                     ), FALSE)
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
                          AND (b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
                               OR v.created_at < COALESCE((
                                   SELECT req.requested_at
                                   FROM book_ordinary_deletion_requests req
                                   WHERE req.book_id=b.id
                                     AND req.request_revision=b.lifecycle_revision
                                       - CASE WHEN b.lifecycle_status='pending_deletion'
                                              THEN 0 ELSE 1 END
                               ),b.ordinary_deleted_at))
                          AND (b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
                               OR (EXISTS (
                                       SELECT 1 FROM commerce_layer_terms legacy
                                       WHERE legacy.user_id=t.user_id
                                         AND legacy.layer_id=t.layer_id
                                         AND legacy.access_origin_version_id=t.access_origin_version_id
                                         AND legacy.purchased_at<COALESCE((
                                             SELECT req.requested_at
                                             FROM book_ordinary_deletion_requests req
                                             WHERE req.book_id=b.id
                                               AND req.request_revision=b.lifecycle_revision
                                       - CASE WHEN b.lifecycle_status='pending_deletion'
                                              THEN 0 ELSE 1 END
                                         ),b.ordinary_deleted_at)
                                         AND (legacy.expires_at IS NULL
                                              OR legacy.expires_at>COALESCE((
                                                  SELECT req.requested_at
                                                  FROM book_ordinary_deletion_requests req
                                                  WHERE req.book_id=b.id
                                                    AND req.request_revision=b.lifecycle_revision
                                       - CASE WHEN b.lifecycle_status='pending_deletion'
                                              THEN 0 ELSE 1 END
                                              ),b.ordinary_deleted_at))
                                   )))
                          AND t.started_at<=now()
                          AND (t.expires_at IS NULL OR t.expires_at>now())
                          AND NOT EXISTS (
                              SELECT 1 FROM commerce_layer_term_revocations r
                              WHERE r.term_id=t.id
                          )
                    ) AS entitled,
                    (b.lifecycle_status='ordinary_deleted') AS archived
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN users AS viewer ON viewer.id=$1
             LEFT JOIN chapter_publications cp ON cp.chapter_id=c.id
             WHERE c.id=$2",
        )
        .bind(actor.user_id.get())
        .bind(chapter_id.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;

        let Some((version, readable, entitled, archived)) = record else {
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
                // Do not let late buyers enumerate a privately retained
                // Book via different 403/404 responses after ordinary delete.
                reason: if archived {
                    DenialReason::Hidden
                } else {
                    DenialReason::NotPurchased
                },
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
