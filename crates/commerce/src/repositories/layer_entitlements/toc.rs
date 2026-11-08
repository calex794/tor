//! D-05: paged Layer CURRENT navigation, no purchase IDs, body or S3 grants.
//! Every paid chapter is independently authorized by the same Layer policy
//! that gates its encrypted delivery, rather than by book membership alone.
use super::*;
use crate::services::reader_views::{
    LayerTocAccess, LayerTocAnchor, LayerTocChapter, LayerTocWindow, MixedTocCursor, MixedTocSide,
};
use sqlx::Row;

impl PgLayerChapterEntitlements {
    /// Resolve the actual chapter number and names only after live D Layer
    /// entitlement for this concrete CURRENT edition has been verified.
    pub async fn toc_anchor(
        &self,
        actor: PositiveId,
        chapter_id: PositiveId,
    ) -> Result<LayerTocAnchor, DeliveryError> {
        match self.resolve_entitlement(Actor { user_id: actor }, chapter_id, None).await? {
            EntitlementDecision::Allowed { pinned } if pinned.chapter_id == chapter_id => {}
            EntitlementDecision::Allowed { .. } => return Err(DeliveryError::Conflict),
            EntitlementDecision::Denied { reason: DenialReason::Hidden } =>
                return Err(DeliveryError::Hidden),
            EntitlementDecision::Denied { .. } => return Err(DeliveryError::Denied),
        }
        let row = sqlx::query(
            "SELECT c.chapter_no, c.title AS chapter_title, b.title AS book_title
             FROM chapters c JOIN books b ON b.id=c.book_id WHERE c.id=$1",
        ).bind(chapter_id.get())
         .fetch_optional(&self.pool).await.map_err(|_| DeliveryError::Unavailable)?
         .ok_or(DeliveryError::Unavailable)?;
        Ok(LayerTocAnchor {
            chapter_id: chapter_id.get(),
            chapter_no: row.try_get("chapter_no").map_err(|_| DeliveryError::Unavailable)?,
            chapter_title: row.try_get("chapter_title").map_err(|_| DeliveryError::Unavailable)?,
            book_title: row.try_get("book_title").map_err(|_| DeliveryError::Unavailable)?,
        })
    }

    /// Keyset page around a D-authorized Layer chapter. An arbitrary cursor
    /// can move within its book but never grant access to another chapter.
    pub async fn layer_toc_window(
        &self,
        actor: PositiveId,
        chapter_id: PositiveId,
        cursor: MixedTocCursor,
        side: MixedTocSide,
        limit: u16,
    ) -> Result<LayerTocWindow, DeliveryError> {
        if !(1..=60).contains(&limit) || cursor.chapter_no <= 0 || cursor.chapter_id <= 0 {
            return Err(DeliveryError::Invalid);
        }
        self.toc_anchor(actor, chapter_id).await?;
        // Only real chapter coordinates inside the anchor book are accepted.
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM chapters anchor
                JOIN chapters c ON c.book_id=anchor.book_id
                WHERE anchor.id=$1 AND c.id=$2 AND c.chapter_no=$3
            )",
        )
        .bind(chapter_id.get()).bind(cursor.chapter_id).bind(cursor.chapter_no)
        .fetch_one(&self.pool).await.map_err(|_| DeliveryError::Unavailable)?;
        if !valid {
            return Err(DeliveryError::Invalid);
        }
        let (comparison, order) = match side {
            MixedTocSide::Before => ("<", "DESC"),
            MixedTocSide::After => (">", "ASC"),
        };
        // B publication/age/visibility guards; no preview or unpublished text.
        // All paid rows below receive a second independent D CURRENT check.
        let sql = format!(
            "SELECT c.id AS chapter_id, c.chapter_no, c.title, c.access_level
             FROM chapters anchor
             JOIN chapters c ON c.book_id=anchor.book_id
             JOIN books b ON b.id=c.book_id
             JOIN users viewer ON viewer.id=$1 AND viewer.account_state='active'
             WHERE anchor.id=$2 AND b.visibility='public'
               AND b.visibility_status<>'blocked'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.access_level='basic'
                 OR (b.access_level='adult' AND viewer.adult_enabled)
                 OR (b.access_level='pro_adult' AND EXISTS (
                    SELECT 1 FROM user_level_grants g
                    WHERE g.user_id=viewer.id AND g.level='pro_adult' AND g.is_active
                 )))
               AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND (c.access_level='free' OR
                    (c.access_level='paid' AND c.price_amount>0))
               AND EXISTS (
                   SELECT 1 FROM chapter_publications publication
                   JOIN chapter_versions v ON v.id=publication.version_id
                       AND v.chapter_id=c.id
                   WHERE publication.chapter_id=c.id
               )
               AND (c.chapter_no,c.id) {comparison} ($3,$4)
             ORDER BY c.chapter_no {order},c.id {order} LIMIT $5"
        );
        let rows = sqlx::query(&sql)
            .bind(actor.get()).bind(chapter_id.get())
            .bind(cursor.chapter_no).bind(cursor.chapter_id)
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool).await.map_err(|_| DeliveryError::Unavailable)?;
        let has_more = rows.len() > usize::from(limit);
        let mut chapters = Vec::with_capacity(usize::from(limit));
        for row in rows.into_iter().take(usize::from(limit)) {
            let id: i64 = row.try_get("chapter_id").map_err(|_| DeliveryError::Unavailable)?;
            let number: i32 = row.try_get("chapter_no").map_err(|_| DeliveryError::Unavailable)?;
            let title: String = row.try_get("title").map_err(|_| DeliveryError::Unavailable)?;
            let kind: String = row.try_get("access_level").map_err(|_| DeliveryError::Unavailable)?;
            let access = if kind == "free" {
                LayerTocAccess::PublicFree
            } else if kind == "paid" {
                let chapter = PositiveId::new(id).map_err(|_| DeliveryError::Unavailable)?;
                match self.resolve_entitlement(Actor { user_id: actor }, chapter, None).await? {
                    EntitlementDecision::Allowed { pinned } if pinned.chapter_id == chapter => {}
                    _ => return Err(DeliveryError::Denied),
                }
                LayerTocAccess::Subscription
            } else {
                return Err(DeliveryError::Unavailable);
            };
            chapters.push(LayerTocChapter { chapter_id: id, chapter_no: number, title, access });
        }
        if matches!(side, MixedTocSide::Before) {
            chapters.reverse();
        }
        let next_cursor = if has_more {
            let edge = if matches!(side, MixedTocSide::Before) {
                chapters.first()
            } else {
                chapters.last()
            };
            edge.map(|c| MixedTocCursor {
                chapter_no: c.chapter_no,
                chapter_id: c.chapter_id,
            }.encode())
        } else {
            None
        };
        Ok(LayerTocWindow { chapters, next_cursor })
    }
}
