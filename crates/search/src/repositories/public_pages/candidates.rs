use super::*;

// The existing anonymous B/C public-reader policy, plus the catalog's pending
// deletion exclusion. SEO indexability is deliberately not an access decision.
const PUBLIC_BOOK: &str = "b.visibility='public' AND b.visibility_status='active'
    AND NOT b.admin_hold AND NOT b.is_deleted AND b.access_level='basic'
    AND b.lifecycle_status<>'pending_deletion'";

pub(super) async fn read(
    tx: &mut Transaction<'_, Postgres>,
    lock: bool,
) -> Result<Vec<PageTarget>, PageProbeError> {
    let mut targets: Vec<_> = PageSlot::ALL.into_iter().map(PageTarget::empty).collect();
    let sql = format!("SELECT b.id,b.xmin::text AS revision FROM books b WHERE {PUBLIC_BOOK}
        ORDER BY b.id LIMIT 1 {}", if lock { "FOR SHARE OF b" } else { "" });
    if let Some(row) = sqlx::query(&sql).fetch_optional(&mut **tx).await.map_err(unavailable)? {
        targets[2].book_id = Some(row.try_get("id").map_err(unavailable)?);
        targets[2].book_revision = Some(row.try_get("revision").map_err(unavailable)?);
    }
    let sql = format!("SELECT b.id AS book_id,c.id AS chapter_id,p.version_id,
        b.xmin::text AS book_revision,c.xmin::text AS chapter_revision,p.xmin::text AS publication_revision
        FROM chapters c JOIN books b ON b.id=c.book_id
        JOIN chapter_publications p ON p.chapter_id=c.id
        JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
        WHERE {PUBLIC_BOOK} AND c.visibility='public' AND c.access_level='free'
        AND NOT c.admin_hold AND NOT c.is_deleted
        AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
        ORDER BY c.id LIMIT 1 {}", if lock { "FOR SHARE OF b,c,p" } else { "" });
    if let Some(row) = sqlx::query(&sql).fetch_optional(&mut **tx).await.map_err(unavailable)? {
        targets[3] = PageTarget {
            slot: PageSlot::FreeChapter,
            book_id: Some(row.try_get("book_id").map_err(unavailable)?),
            chapter_id: Some(row.try_get("chapter_id").map_err(unavailable)?),
            version_id: Some(row.try_get("version_id").map_err(unavailable)?),
            book_revision: Some(row.try_get("book_revision").map_err(unavailable)?),
            chapter_revision: Some(row.try_get("chapter_revision").map_err(unavailable)?),
            publication_revision: Some(row.try_get("publication_revision").map_err(unavailable)?),
        };
    }
    Ok(targets)
}
