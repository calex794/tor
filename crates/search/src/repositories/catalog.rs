//! F-02: PostgreSQL keyset catalog. All predicates precede pagination.
//! Oldest and newest use fixed SQL, never a client-provided ORDER BY fragment.
use crate::domain::{CatalogSort, SearchBook, SearchError, SearchPage};
use sqlx::{PgPool, Row};

const ASCENDING: &str = "
SELECT id,title,target_language,author_name,genres FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, title) || to_tsvector('simple'::regconfig, author_name))
           @@ plainto_tsquery('simple'::regconfig, $1))
  AND id > $2
  AND ($4::text IS NULL OR target_language = $4)
  AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
ORDER BY id ASC LIMIT $3";

const DESCENDING: &str = "
SELECT id,title,target_language,author_name,genres FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, title) || to_tsvector('simple'::regconfig, author_name))
           @@ plainto_tsquery('simple'::regconfig, $1))
  AND ($2::bigint = 0 OR id < $2)
  AND ($4::text IS NULL OR target_language = $4)
  AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
ORDER BY id DESC LIMIT $3";

pub(super) async fn read_page(
    pool: &PgPool,
    term: &str,
    language: Option<&str>,
    genre: Option<&str>,
    after: i64,
    limit: usize,
    sort: CatalogSort,
) -> Result<SearchPage, SearchError> {
    let sql = match sort {
        CatalogSort::Oldest => ASCENDING,
        CatalogSort::Newest => DESCENDING,
    };
    let rows = sqlx::query(sql)
        .bind(term.trim())
        .bind(after)
        .bind(i64::try_from(limit + 1).map_err(|_| SearchError::Invalid)?)
        .bind(language)
        .bind(genre)
        .fetch_all(pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;

    let mut items = rows
        .into_iter()
        .map(|row| {
            let id: i64 = row.try_get("id").map_err(|_| SearchError::Unavailable)?;
            Ok(SearchBook {
                id: u64::try_from(id).map_err(|_| SearchError::Unavailable)?,
                title: row.try_get("title").map_err(|_| SearchError::Unavailable)?,
                language: row
                    .try_get("target_language")
                    .map_err(|_| SearchError::Unavailable)?,
                author_name: row
                    .try_get("author_name")
                    .map_err(|_| SearchError::Unavailable)?,
                genres: row
                    .try_get("genres")
                    .map_err(|_| SearchError::Unavailable)?,
            })
        })
        .collect::<Result<Vec<_>, SearchError>>()?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|book| book.id.to_string())
    } else {
        None
    };
    Ok(SearchPage { items, next_cursor })
}

/// Exact count of the same public, non-held, non-deleted search projection.
/// This query is separate from the hot keyset path and must be opt-in.
const FILTERED_TOTAL: &str = "
SELECT COUNT(*) FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, title) || to_tsvector('simple'::regconfig, author_name))
           @@ plainto_tsquery('simple'::regconfig, $1))
  AND ($2::text IS NULL OR target_language = $2)
  AND ($3::text IS NULL OR genres @> ARRAY[$3::text])";

pub(super) async fn count_filtered(
    pool: &PgPool,
    term: &str,
    language: Option<&str>,
    genre: Option<&str>,
) -> Result<u64, SearchError> {
    let total: i64 = sqlx::query_scalar(FILTERED_TOTAL)
        .bind(term.trim())
        .bind(language)
        .bind(genre)
        .fetch_one(pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
    u64::try_from(total).map_err(|_| SearchError::Unavailable)
}
