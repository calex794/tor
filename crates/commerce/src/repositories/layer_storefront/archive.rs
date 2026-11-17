//! D-05: private, bounded archive summary for already paid Layer terms.
//! A historical Book title never comes from the public Layer storefront.
//! Only live, actor-owned pre-deletion terms can enumerate its paid chapters.
use super::{ArchivedLayerBook, LayerTermCard};
use crate::domain::layers::LayerError;
use sqlx::{PgPool, Row};
use std::collections::HashMap;

pub(super) async fn list_owned_archived_books(
    actor: i64,
    pool: &PgPool,
    terms: &[LayerTermCard],
) -> Result<HashMap<i64, Vec<ArchivedLayerBook>>, LayerError> {
    let ids: Vec<i64> = terms.iter().map(|term| term.term_id).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    // At most 50 terms are shown by /me/library/layers. Pick one actually
    // published paid chapter for each archived Book; the chapter reader will
    // still verify D, B and G before returning encrypted bytes.
    let rows = sqlx::query(
        "SELECT DISTINCT ON (t.id,b.id)
                t.id AS term_id,b.id AS book_id,b.title,c.id AS chapter_id
         FROM commerce_layer_terms t
         JOIN users viewer ON viewer.id=t.user_id
              AND viewer.account_state='active'
         JOIN books b ON b.lifecycle_status='ordinary_deleted'
              AND b.ordinary_deleted_at IS NOT NULL
              AND b.visibility_status='active'
              AND NOT b.admin_hold AND NOT b.is_deleted
         JOIN chapters c ON c.book_id=b.id AND c.visibility='public'
              AND c.access_level='paid' AND c.price_amount>0
              AND NOT c.admin_hold AND NOT c.is_deleted
              AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
         JOIN chapter_publications cp ON cp.chapter_id=c.id
         WHERE t.user_id=$1 AND t.id=ANY($2::bigint[])
           AND t.purchased_at<COALESCE((
               SELECT req.requested_at
               FROM book_ordinary_deletion_requests req
               WHERE req.book_id=b.id
                 AND req.request_revision=b.lifecycle_revision-1
           ),b.ordinary_deleted_at)
           AND t.started_at<=now()
           AND (t.expires_at IS NULL OR t.expires_at>now())
           AND NOT EXISTS (
               SELECT 1 FROM commerce_layer_term_revocations r
               WHERE r.term_id=t.id
           )
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
               SELECT 1 FROM commerce_layer_versions origin
               JOIN commerce_layer_versions v
                 ON v.layer_id=origin.layer_id
                AND v.version_no>=origin.version_no
                AND (t.expires_at IS NULL OR v.created_at<t.expires_at)
               JOIN commerce_layer_version_books vb
                 ON vb.layer_version_id=v.id AND vb.book_id=b.id
               WHERE origin.id=t.access_origin_version_id
                 AND origin.layer_id=t.layer_id
           )
         ORDER BY t.id,b.id,c.chapter_no,c.id LIMIT 5000",
    )
    .bind(actor)
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|_| LayerError::Unavailable)?;
    let mut result: HashMap<i64, Vec<ArchivedLayerBook>> = HashMap::new();
    for row in rows {
        let term: i64 = row
            .try_get("term_id")
            .map_err(|_| LayerError::Unavailable)?;
        result.entry(term).or_default().push(ArchivedLayerBook {
            book_id: row
                .try_get("book_id")
                .map_err(|_| LayerError::Unavailable)?,
            title: row.try_get("title").map_err(|_| LayerError::Unavailable)?,
            chapter_id: row
                .try_get("chapter_id")
                .map_err(|_| LayerError::Unavailable)?,
        });
    }
    Ok(result)
}
