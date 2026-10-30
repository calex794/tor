//! F-02: PostgreSQL keyset catalog. All predicates precede pagination.
//! Oldest and newest use fixed SQL, never a client-provided ORDER BY fragment.
use super::CatalogFacets;
use crate::domain::{CatalogSort, SearchBook, SearchError, SearchPage};
use sqlx::{PgPool, Row};

const ASCENDING: &str = "
SELECT id,title,target_language,author_name,genres,tags,fandoms FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND id > $2
  AND ($4::text IS NULL OR target_language = $4)
  AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
  AND ($6::text IS NULL OR tags @> ARRAY[$6::text])
  AND ($7::text IS NULL OR fandoms @> ARRAY[$7::text])
ORDER BY id ASC LIMIT $3";

const DESCENDING: &str = "
SELECT id,title,target_language,author_name,genres,tags,fandoms FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND ($2::bigint = 0 OR id < $2)
  AND ($4::text IS NULL OR target_language = $4)
  AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
  AND ($6::text IS NULL OR tags @> ARRAY[$6::text])
  AND ($7::text IS NULL OR fandoms @> ARRAY[$7::text])
ORDER BY id DESC LIMIT $3";

pub(super) async fn read_page(
    pool: &PgPool,
    filters: CatalogFacets<'_>,
    after: i64,
    limit: usize,
    sort: CatalogSort,
) -> Result<SearchPage, SearchError> {
    let sql = match sort {
        CatalogSort::Oldest => ASCENDING,
        CatalogSort::Newest => DESCENDING,
        _ => return Err(SearchError::Invalid),
    };
    let rows = sqlx::query(sql)
        .bind(filters.term.trim())
        .bind(after)
        .bind(i64::try_from(limit + 1).map_err(|_| SearchError::Invalid)?)
        .bind(filters.language)
        .bind(filters.genre)
        .bind(filters.tag)
        .bind(filters.fandom)
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
                tags: row.try_get("tags").map_err(|_| SearchError::Unavailable)?,
                fandoms: row
                    .try_get("fandoms")
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
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND ($2::text IS NULL OR target_language = $2)
  AND ($3::text IS NULL OR genres @> ARRAY[$3::text])
  AND ($4::text IS NULL OR tags @> ARRAY[$4::text])
  AND ($5::text IS NULL OR fandoms @> ARRAY[$5::text])";

pub(super) async fn count_filtered(
    pool: &PgPool,
    term: &str,
    language: Option<&str>,
    genre: Option<&str>,
    tag: Option<&str>,
    fandom: Option<&str>,
) -> Result<u64, SearchError> {
    // Exact, transactionally maintained public total for unfiltered pages.
    // Filtered totals still use the original SQL predicates, never estimates.
    if term.trim().is_empty()
        && language.is_none()
        && genre.is_none()
        && tag.is_none()
        && fandom.is_none()
    {
        let count: i64 = sqlx::query_scalar(
            "SELECT visible_book_count FROM public_catalog_statistics WHERE singleton",
        )
        .fetch_one(pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
        return u64::try_from(count).map_err(|_| SearchError::Unavailable);
    }
    let total: i64 = sqlx::query_scalar(FILTERED_TOTAL)
        .bind(term.trim())
        .bind(language)
        .bind(genre)
        .bind(tag)
        .bind(fandom)
        .fetch_one(pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
    u64::try_from(total).map_err(|_| SearchError::Unavailable)
}

// F-02 parity with the NEwBOO catalogue: title/author use composite keysets.
// The extra id is the stable tie breaker for duplicate names.
const TITLE_ORDER: &str = "
SELECT id,title,target_language,author_name,genres,tags,fandoms,lower(title) AS sort_key FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND ($2::text IS NULL OR (lower(title),id) > ($2::text,$3::bigint))
  AND ($5::text IS NULL OR target_language=$5)
  AND ($6::text IS NULL OR genres @> ARRAY[$6::text])
  AND ($7::text IS NULL OR tags @> ARRAY[$7::text])
  AND ($8::text IS NULL OR fandoms @> ARRAY[$8::text])
ORDER BY lower(title) ASC,id ASC LIMIT $4";

const AUTHOR_ORDER: &str = "
SELECT id,title,target_language,author_name,genres,tags,fandoms,lower(author_name) AS sort_key FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND ($2::text IS NULL OR (lower(author_name),id) > ($2::text,$3::bigint))
  AND ($5::text IS NULL OR target_language=$5)
  AND ($6::text IS NULL OR genres @> ARRAY[$6::text])
  AND ($7::text IS NULL OR tags @> ARRAY[$7::text])
  AND ($8::text IS NULL OR fandoms @> ARRAY[$8::text])
ORDER BY lower(author_name) ASC,id ASC LIMIT $4";

fn hex_nibble(byte: u8) -> Result<u8, SearchError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(SearchError::Invalid),
    }
}

/// The key is taken from PostgreSQL lower(...), not Rust Unicode lowercasing.
/// This keeps the cursor's collation consistent with ORDER BY in PostgreSQL.
fn encode_lexical_cursor(sort: CatalogSort, key: &str, id: u64) -> String {
    let mut encoded = String::with_capacity(key.len() * 2 + 40);
    encoded.push_str(sort.as_str());
    encoded.push('.');
    for byte in key.bytes() {
        encoded.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
        encoded.push(char::from(b"0123456789abcdef"[(byte & 15) as usize]));
    }
    encoded.push('.');
    encoded.push_str(&id.to_string());
    encoded
}

fn decode_lexical_cursor(
    raw: Option<&str>,
    sort: CatalogSort,
) -> Result<(Option<String>, i64), SearchError> {
    let Some(raw) = raw else {
        return Ok((None, 0));
    };
    if raw.len() > 2048 {
        return Err(SearchError::Invalid);
    }
    let mut parts = raw.split('.');
    let (Some(prefix), Some(hex), Some(id), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(SearchError::Invalid);
    };
    if prefix != sort.as_str()
        || hex.len() % 2 != 0
        || id.is_empty()
        || id.starts_with('0')
        || !id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(SearchError::Invalid);
    }
    let id: i64 = id.parse().map_err(|_| SearchError::Invalid)?;
    if id <= 0 {
        return Err(SearchError::Invalid);
    }
    let mut decoded = Vec::with_capacity(hex.len() / 2);
    for digits in hex.as_bytes().as_chunks::<2>().0 {
        decoded.push((hex_nibble(digits[0])? << 4) | hex_nibble(digits[1])?);
    }
    let key = String::from_utf8(decoded).map_err(|_| SearchError::Invalid)?;
    Ok((Some(key), id))
}

pub(super) async fn read_lexical_page(
    pool: &PgPool,
    filters: CatalogFacets<'_>,
    after: Option<&str>,
    limit: usize,
    sort: CatalogSort,
) -> Result<SearchPage, SearchError> {
    let query = match sort {
        CatalogSort::Title => TITLE_ORDER,
        CatalogSort::Author => AUTHOR_ORDER,
        _ => return Err(SearchError::Invalid),
    };
    let (cursor_key, cursor_id) = decode_lexical_cursor(after, sort)?;
    let rows = sqlx::query(query)
        .bind(filters.term.trim())
        .bind(cursor_key.as_deref())
        .bind(cursor_id)
        .bind(i64::try_from(limit + 1).map_err(|_| SearchError::Invalid)?)
        .bind(filters.language)
        .bind(filters.genre)
        .bind(filters.tag)
        .bind(filters.fandom)
        .fetch_all(pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
    let mut entries = rows
        .into_iter()
        .map(|row| {
            let id: i64 = row.try_get("id").map_err(|_| SearchError::Unavailable)?;
            let book = SearchBook {
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
                tags: row.try_get("tags").map_err(|_| SearchError::Unavailable)?,
                fandoms: row
                    .try_get("fandoms")
                    .map_err(|_| SearchError::Unavailable)?,
            };
            let key: String = row
                .try_get("sort_key")
                .map_err(|_| SearchError::Unavailable)?;
            Ok((book, key))
        })
        .collect::<Result<Vec<_>, SearchError>>()?;
    let next_cursor = if entries.len() > limit {
        entries.truncate(limit);
        entries
            .last()
            .map(|(book, key)| encode_lexical_cursor(sort, key, book.id))
    } else {
        None
    };
    Ok(SearchPage {
        items: entries.into_iter().map(|(book, _)| book).collect(),
        next_cursor,
    })
}

/// B-owned taxonomy options available only from published, unblocked books.
#[derive(Clone, Debug, Default)]
pub struct TaxonomyOptions {
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub fandoms: Vec<String>,
}

pub(super) async fn taxonomy_options(pool: &PgPool) -> Result<TaxonomyOptions, SearchError> {
    // The migration maintains exact distinct-public-book references in the
    // SAME transaction as every publication, hold, deletion and facet change.
    // No stale TTL, pg_class estimates, or per-request scans of all books.
    const SQL: &str = "
        SELECT 'genre' AS kind, name FROM (
            SELECT name FROM public_catalog_facets
            WHERE kind='genre' AND reference_count>0
            ORDER BY name LIMIT 100
        ) g
        UNION ALL
        SELECT 'tag' AS kind, name FROM (
            SELECT name FROM public_catalog_facets
            WHERE kind='tag' AND reference_count>0
            ORDER BY name LIMIT 100
        ) t
        UNION ALL
        SELECT 'fandom' AS kind, name FROM (
            SELECT name FROM public_catalog_facets
            WHERE kind='fandom' AND reference_count>0
            ORDER BY name LIMIT 100
        ) f";

    let rows = sqlx::query(SQL)
        .fetch_all(pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
    let mut result = TaxonomyOptions::default();
    for row in rows {
        let kind: String = row.try_get("kind").map_err(|_| SearchError::Unavailable)?;
        let label: String = row.try_get("name").map_err(|_| SearchError::Unavailable)?;
        match kind.as_str() {
            "genre" => result.genres.push(label),
            "tag" => result.tags.push(label),
            "fandom" => result.fandoms.push(label),
            _ => return Err(SearchError::Unavailable),
        }
    }
    Ok(result)
}
