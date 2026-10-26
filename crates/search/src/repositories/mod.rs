pub mod showcases;

use crate::domain::{validate_facets, SearchBook, SearchError, SearchPage};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgSearch {
    pool: PgPool,
}
impl PgSearch {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn search(
        &self,
        term: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SearchPage, SearchError> {
        self.search_filtered(term, None, after, limit).await
    }

    /// Optional language filter is checked independently of the query text.
    pub async fn search_filtered(
        &self,
        term: &str,
        language: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SearchPage, SearchError> {
        self.search_faceted(term, language, None, after, limit)
            .await
    }

    pub async fn search_faceted(
        &self,
        term: &str,
        language: Option<&str>,
        genre: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SearchPage, SearchError> {
        let (after, language, genre) = validate_facets(term, language, genre, after, limit)?;
        // Title and author are tokenized separately: an unclosed HTML tag in
        // a user-provided title must not hide the author's searchable tokens.
        // Exclude private, held and deleted rows BEFORE applying the page limit.
        // Query parameters are bound, never interpolated into SQL or HTML.
        let rows = sqlx::query(
            "SELECT id,title,target_language,author_name,genres FROM books
             WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
               AND ($1::text = '' OR
                   (to_tsvector('simple'::regconfig, title) || to_tsvector('simple'::regconfig, author_name))
                       @@ plainto_tsquery('simple'::regconfig,$1))
               AND id > $2
               AND ($4::text IS NULL OR target_language = $4)
               AND ($5::text IS NULL OR genres @> ARRAY[$5::text])
             ORDER BY id ASC LIMIT $3",
        )
        .bind(term.trim())
        .bind(after)
        .bind(i64::try_from(limit + 1).map_err(|_| SearchError::Invalid)?)
        .bind(language.as_deref())
        .bind(genre.as_deref())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
        let mut items: Vec<SearchBook> = rows
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
            .collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit {
            items.truncate(limit);
            items.last().map(|book| book.id.to_string())
        } else {
            None
        };
        Ok(SearchPage { items, next_cursor })
    }
}
