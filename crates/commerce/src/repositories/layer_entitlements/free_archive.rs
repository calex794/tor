//! D-05: actor-scoped historical FREE edition identity for an ordinarily
//! removed Book acquired through an uninterrupted paid Layer origin.
//!
//! This is a *version resolver*, not a signed URL, TOC or public entitlement.
//! No direct V1 purchase and no saved-free pin are created.
use super::PgLayerChapterEntitlements;
use sqlx::query_scalar;
use tor_contracts::paid_delivery::{DeliveryError, PositiveId};

impl PgLayerChapterEntitlements {
    /// Only an immutable, pre-deletion published free chapter may become a
    /// private archive candidate. Every call checks live term and moderation
    /// status; callers MUST recheck after S3 signing before releasing a grant.
    pub async fn archived_free_version(
        &self,
        actor: PositiveId,
        chapter: PositiveId,
    ) -> Result<Option<PositiveId>, DeliveryError> {
        let version: Option<i64> = query_scalar(
            "SELECT snapshot.version_id
             FROM book_layer_free_archive_versions snapshot
             JOIN chapters c ON c.id=snapshot.chapter_id
               AND c.book_id=snapshot.book_id
             JOIN chapter_versions frozen ON frozen.id=snapshot.version_id
               AND frozen.chapter_id=c.id
             JOIN books b ON b.id=c.book_id
             JOIN book_ordinary_deletion_requests req
               ON req.book_id=b.id AND req.request_revision=snapshot.request_revision
             JOIN users viewer ON viewer.id=$1 AND viewer.account_state='active'
             WHERE c.id=$2 AND c.access_level='free' AND c.price_amount=0
               AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
               AND (
                 (b.lifecycle_status='pending_deletion'
                  AND b.lifecycle_revision=req.request_revision)
                 OR
                 (b.lifecycle_status='ordinary_deleted'
                  AND b.ordinary_deleted_at IS NOT NULL
                  AND b.lifecycle_revision=req.request_revision+1)
               )
               AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (
                    b.access_level='basic'
                    OR (b.access_level='adult' AND viewer.adult_enabled)
                    OR (b.access_level='pro_adult' AND EXISTS (
                        SELECT 1 FROM user_level_grants age
                        WHERE age.user_id=viewer.id AND age.level='pro_adult'
                          AND age.is_active
                    ))
               )
               AND EXISTS (
                    SELECT 1 FROM commerce_layer_terms t
                    JOIN commerce_layer_versions origin
                      ON origin.id=t.access_origin_version_id
                     AND origin.layer_id=t.layer_id
                    JOIN commerce_layer_versions v ON v.layer_id=t.layer_id
                     AND v.version_no>=origin.version_no
                     AND (t.expires_at IS NULL OR v.created_at<t.expires_at)
                    JOIN commerce_layer_version_books vb
                      ON vb.layer_version_id=v.id AND vb.book_id=b.id
                    WHERE t.user_id=viewer.id
                      AND v.created_at<req.requested_at
                      AND t.started_at<=now()
                      AND (t.expires_at IS NULL OR t.expires_at>now())
                      AND NOT EXISTS (
                          SELECT 1 FROM commerce_layer_term_revocations r
                          WHERE r.term_id=t.id
                      )
                      AND EXISTS (
                          SELECT 1 FROM commerce_layer_terms legacy
                          WHERE legacy.user_id=t.user_id
                            AND legacy.layer_id=t.layer_id
                            AND legacy.access_origin_version_id=t.access_origin_version_id
                            AND legacy.purchased_at<req.requested_at
                            AND (legacy.expires_at IS NULL
                                 OR legacy.expires_at>req.requested_at)
                      )
               )
             ORDER BY snapshot.request_revision DESC LIMIT 1",
        )
        .bind(actor.get())
        .bind(chapter.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        version
            .map(|id| PositiveId::new(id).map_err(|_| DeliveryError::Unavailable))
            .transpose()
    }
}
