//! D-scoped mixed table of contents: bounded keyset navigation without paid text.
//! The anchor purchase is checked on every query. Public rows use B publication
//! predicates; inaccessible private rows are never enumerated.
use super::*;
use crate::services::reader_views::{MixedTocCursor, MixedTocSide, MixedTocWindow};
use sqlx::postgres::PgRow;

const ELIGIBLE: &str = r#"
WITH anchor AS (
    SELECT c.book_id
    FROM library_purchased_chapters l
    JOIN chapter_purchase_facts p
      ON p.id=l.purchase_id AND p.user_id=l.user_id
     AND p.chapter_id=l.chapter_id AND p.version_id=l.active_version_id
    JOIN chapters c ON c.id=p.chapter_id
    WHERE l.user_id=$1 AND p.chapter_id=$2 AND p.id=$3
      AND tor_purchase_version_readable($1,p.id,p.chapter_id,p.version_id)
)
SELECT c.id AS chapter_id, c.chapter_no, c.title AS chapter_title,
       c.access_level, c.price_amount,
       owned.purchase_id, owned.version_id, owned.version_no
FROM anchor a
JOIN chapters c ON c.book_id=a.book_id
JOIN books b ON b.id=c.book_id
LEFT JOIN LATERAL (
    SELECT p.id AS purchase_id, p.version_id, v.version_no
    FROM library_purchased_chapters l
    JOIN chapter_purchase_facts p
      ON p.id=l.purchase_id AND p.user_id=l.user_id
     AND p.chapter_id=l.chapter_id AND p.version_id=l.active_version_id
    JOIN chapter_versions v
      ON v.chapter_id=p.chapter_id AND v.id=p.version_id
    WHERE l.user_id=$1 AND l.chapter_id=c.id
      AND tor_purchase_version_readable($1,p.id,p.chapter_id,p.version_id)
    LIMIT 1
) owned ON TRUE
WHERE (owned.purchase_id IS NOT NULL
    OR (b.visibility='public' AND b.visibility_status='active'
      AND NOT b.admin_hold AND NOT b.is_deleted
      AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
      AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
      AND EXISTS (
        SELECT 1 FROM chapter_publications cp
        JOIN chapter_versions pv
          ON pv.chapter_id=cp.chapter_id AND pv.id=cp.version_id
        WHERE cp.chapter_id=c.id
      )))
"#;

fn chapter_from_row(row: PgRow) -> Result<MixedTocChapter, DeliveryError> {
    let chapter_id: i64 = row
        .try_get("chapter_id")
        .map_err(|_| DeliveryError::Unavailable)?;
    let chapter_no: i32 = row
        .try_get("chapter_no")
        .map_err(|_| DeliveryError::Unavailable)?;
    let title: String = row
        .try_get("chapter_title")
        .map_err(|_| DeliveryError::Unavailable)?;
    let owned: Option<i64> = row
        .try_get("purchase_id")
        .map_err(|_| DeliveryError::Unavailable)?;
    let access = if let Some(purchase_id) = owned {
        MixedTocAccess::Purchased {
            purchase_id,
            version_id: row
                .try_get("version_id")
                .map_err(|_| DeliveryError::Unavailable)?,
            version_no: row
                .try_get("version_no")
                .map_err(|_| DeliveryError::Unavailable)?,
        }
    } else {
        let kind: String = row
            .try_get("access_level")
            .map_err(|_| DeliveryError::Unavailable)?;
        if kind == "free" {
            MixedTocAccess::PublicFree
        } else {
            MixedTocAccess::Locked {
                price_minor: row
                    .try_get("price_amount")
                    .map_err(|_| DeliveryError::Unavailable)?,
            }
        }
    };
    Ok(MixedTocChapter {
        chapter_id,
        chapter_no,
        title,
        access,
    })
}

impl PgDirectEntitlements {
    /// A cursor from this book must still be a currently visible public row or
    /// a D-readable purchase. Neither the cursor nor the request grants access.
    pub async fn mixed_book_toc_window(
        &self,
        actor: PositiveId,
        anchor_chapter: PositiveId,
        anchor_purchase: PositiveId,
        cursor: MixedTocCursor,
        side: MixedTocSide,
        limit: u16,
    ) -> Result<MixedTocWindow, DeliveryError> {
        if !(1..=100).contains(&limit) || cursor.chapter_no <= 0 || cursor.chapter_id <= 0 {
            return Err(DeliveryError::Invalid);
        }
        let validate = format!("{ELIGIBLE} AND c.chapter_no=$4 AND c.id=$5 LIMIT 1");
        let valid = sqlx::query(&validate)
            .bind(actor.get())
            .bind(anchor_chapter.get())
            .bind(anchor_purchase.get())
            .bind(cursor.chapter_no)
            .bind(cursor.chapter_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| DeliveryError::Unavailable)?
            .is_some();
        if !valid {
            return Err(DeliveryError::Invalid);
        }
        let (comparison, order) = match side {
            MixedTocSide::Before => ("<", "DESC"),
            MixedTocSide::After => (">", "ASC"),
        };
        let statement = format!(
            "{ELIGIBLE} AND (c.chapter_no, c.id) {comparison} ($4, $5) \
             ORDER BY c.chapter_no {order}, c.id {order} LIMIT $6"
        );
        let rows = sqlx::query(&statement)
            .bind(actor.get())
            .bind(anchor_chapter.get())
            .bind(anchor_purchase.get())
            .bind(cursor.chapter_no)
            .bind(cursor.chapter_id)
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        let has_more = rows.len() > usize::from(limit);
        let mut chapters = rows
            .into_iter()
            .take(usize::from(limit))
            .map(chapter_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if matches!(side, MixedTocSide::Before) {
            chapters.reverse();
        }
        let next_cursor = if has_more {
            let edge = if matches!(side, MixedTocSide::Before) {
                chapters.first()
            } else {
                chapters.last()
            };
            edge.map(|row| {
                MixedTocCursor {
                    chapter_no: row.chapter_no,
                    chapter_id: row.chapter_id,
                }
                .encode()
            })
        } else {
            None
        };
        Ok(MixedTocWindow {
            chapters,
            next_cursor,
        })
    }
}
