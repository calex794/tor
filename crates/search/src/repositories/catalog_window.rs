//! B-18: previous/next catalog pages without OFFSET or client cursor stacks.
//! Visibility and bound facets are identical to the public F-02 search.
//! A "before" request runs the reverse indexed keyset, then restores display order.
use super::{catalog, CatalogFacets, PgSearch};
use crate::domain::{
    normalize_genre, validate_browse, CatalogSort, SearchBook, SearchError,
};
use sqlx::{PgPool, Row};

pub struct CatalogWindow {
    pub items: Vec<SearchBook>,
    pub next_cursor: Option<String>,
    pub previous_cursor: Option<String>,
}

impl PgSearch {
    /// Browser-only navigation. The public JSON API retains its old shape.
    /// Each request reads at most limit+1 books; no OFFSET and no cursor trail.
    pub async fn browse_catalog_page(
        &self,
        facets: CatalogFacets<'_>,
        after: Option<&str>,
        before: Option<&str>,
        limit: usize,
        sort: Option<&str>,
    ) -> Result<CatalogWindow, SearchError> {
        if after.is_some() && before.is_some() {
            return Err(SearchError::Invalid);
        }
        let order = CatalogSort::parse(sort)?;
        if let Some(anchor) = before {
            return read_previous_page(&self.pool, facets, anchor, limit, order).await;
        }
        let page = self.search_catalog(facets, after, limit, sort).await?;
        let previous_cursor = if after.is_some() {
            match page.items.first() {
                Some(first) => Some(cursor_for_book(&self.pool, first, order).await?),
                None => None,
            }
        } else {
            None
        };
        Ok(CatalogWindow {
            items: page.items,
            next_cursor: page.next_cursor,
            previous_cursor,
        })
    }
}

/// The lexical key must be PostgreSQL lower(), not Rust's Unicode lower().
async fn cursor_for_book(
    pool: &PgPool,
    first: &SearchBook,
    sort: CatalogSort,
) -> Result<String, SearchError> {
    match sort {
        CatalogSort::Oldest | CatalogSort::Newest => Ok(first.id.to_string()),
        CatalogSort::Title | CatalogSort::Author => {
            let sql = if sort == CatalogSort::Title {
                "SELECT lower(title) FROM books WHERE id=$1"
            } else {
                "SELECT lower(author_name) FROM books WHERE id=$1"
            };
            let key: String = sqlx::query_scalar(sql)
                .bind(i64::try_from(first.id).map_err(|_| SearchError::Unavailable)?)
                .fetch_one(pool)
                .await
                .map_err(|_| SearchError::Unavailable)?;
            Ok(catalog::encode_lexical_cursor(sort, &key, first.id))
        }
    }
}

const PREVIOUS_OLDEST: &str = "
SELECT id,title,target_language,author_name,genres,tags,fandoms FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND id < $2
  AND ($4::text IS NULL OR target_language = $4)
  AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
  AND ($6::text IS NULL OR tags @> ARRAY[$6::text])
  AND ($7::text IS NULL OR fandoms @> ARRAY[$7::text])
ORDER BY id DESC LIMIT $3";

const PREVIOUS_NEWEST: &str = "
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

const PREVIOUS_TITLE: &str = "
SELECT id,title,target_language,author_name,genres,tags,fandoms,lower(title) AS sort_key FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND (lower(title),id) < ($2::text,$3::bigint)
  AND ($5::text IS NULL OR target_language=$5)
  AND ($6::text IS NULL OR genres @> ARRAY[$6::text])
  AND ($7::text IS NULL OR tags @> ARRAY[$7::text])
  AND ($8::text IS NULL OR fandoms @> ARRAY[$8::text])
ORDER BY lower(title) DESC,id DESC LIMIT $4";

const PREVIOUS_AUTHOR: &str = "
SELECT id,title,target_language,author_name,genres,tags,fandoms,lower(author_name) AS sort_key FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
  AND ($1::text = '' OR
       (to_tsvector('simple'::regconfig, translate(lower(normalize(title, NFKC)), 'ё', 'е')) || to_tsvector('simple'::regconfig, translate(lower(normalize(author_name, NFKC)), 'ё', 'е')))
           @@ plainto_tsquery('simple'::regconfig, translate(lower(normalize($1::text, NFKC)), 'ё', 'е')))
  AND (lower(author_name),id) < ($2::text,$3::bigint)
  AND ($5::text IS NULL OR target_language=$5)
  AND ($6::text IS NULL OR genres @> ARRAY[$6::text])
  AND ($7::text IS NULL OR tags @> ARRAY[$7::text])
  AND ($8::text IS NULL OR fandoms @> ARRAY[$8::text])
ORDER BY lower(author_name) DESC,id DESC LIMIT $4";

async fn read_previous_page(
    pool: &PgPool,
    filters: CatalogFacets<'_>,
    before: &str,
    limit: usize,
    sort: CatalogSort,
) -> Result<CatalogWindow, SearchError> {
    let lexical = matches!(sort, CatalogSort::Title | CatalogSort::Author);
    let (_, language, genre) = validate_browse(
        filters.term,
        filters.language,
        filters.genre,
        if lexical { None } else { Some(before) },
        limit,
    )?;
    let tag = normalize_genre(filters.tag)?;
    let fandom = normalize_genre(filters.fandom)?;
    let size = i64::try_from(limit + 1).map_err(|_| SearchError::Invalid)?;
    let rows = (if lexical {
        let (key, id) = catalog::decode_lexical_cursor(Some(before), sort)?;
        let key = key.ok_or(SearchError::Invalid)?;
        let sql = if sort == CatalogSort::Title {
            PREVIOUS_TITLE
        } else {
            PREVIOUS_AUTHOR
        };
        sqlx::query(sql)
            .bind(filters.term.trim())
            .bind(key)
            .bind(id)
            .bind(size)
            .bind(language.as_deref())
            .bind(genre.as_deref())
            .bind(tag.as_deref())
            .bind(fandom.as_deref())
            .fetch_all(pool)
            .await
    } else {
        let (id, _, _) = validate_browse(
            filters.term,
            filters.language,
            filters.genre,
            Some(before),
            limit,
        )?;
        let sql = if sort == CatalogSort::Oldest {
            PREVIOUS_OLDEST
        } else {
            PREVIOUS_NEWEST
        };
        sqlx::query(sql)
            .bind(filters.term.trim())
            .bind(id)
            .bind(size)
            .bind(language.as_deref())
            .bind(genre.as_deref())
            .bind(tag.as_deref())
            .bind(fandom.as_deref())
            .fetch_all(pool)
            .await
    })
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
                genres: row.try_get("genres").map_err(|_| SearchError::Unavailable)?,
                tags: row.try_get("tags").map_err(|_| SearchError::Unavailable)?,
                fandoms: row.try_get("fandoms").map_err(|_| SearchError::Unavailable)?,
            };
            let cursor = if lexical {
                let key: String = row.try_get("sort_key").map_err(|_| SearchError::Unavailable)?;
                catalog::encode_lexical_cursor(sort, &key, book.id)
            } else {
                book.id.to_string()
            };
            Ok((book, cursor))
        })
        .collect::<Result<Vec<_>, SearchError>>()?;
    let has_more_before = entries.len() > limit;
    entries.truncate(limit);
    entries.reverse();
    let previous_cursor = if has_more_before {
        entries.first().map(|(_, cursor)| cursor.clone())
    } else {
        None
    };
    // "before" proves that a following page exists; the last book's cursor
    // takes the user forward again without a stored cursor history.
    let next_cursor = entries.last().map(|(_, cursor)| cursor.clone());
    Ok(CatalogWindow {
        items: entries.into_iter().map(|(book, _)| book).collect(),
        next_cursor,
        previous_cursor,
    })
}
