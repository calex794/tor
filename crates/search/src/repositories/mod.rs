use crate::domain::{validate, SearchBook, SearchError, SearchPage};
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
        let after = validate(term, after, limit)?;
        // Exclude private, held and deleted rows BEFORE applying the page limit.
        // Query parameters are bound, never interpolated into SQL or HTML.
        let rows = sqlx::query(
            "SELECT id,title,target_language FROM books
             WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
             AND to_tsvector('simple'::regconfig, title) @@ plainto_tsquery('simple'::regconfig,$1)
             AND id > $2
             ORDER BY id ASC LIMIT $3",
        )
        .bind(term.trim())
        .bind(after)
        .bind(i64::try_from(limit + 1).map_err(|_| SearchError::Invalid)?)
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
