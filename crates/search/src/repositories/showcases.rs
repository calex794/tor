//! F-04: bounded public-only PostgreSQL projection for homepage/genre spotlights.
use crate::{
    domain::{SearchBook, SearchError},
    services::showcases::ShowcaseRepository,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgShowcases {
    pool: PgPool,
}

impl PgShowcases {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ShowcaseRepository for PgShowcases {
    async fn latest_public(
        &self,
        genre: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchBook>, SearchError> {
        // The partial books_public_id_idx supports descending ID order.
        // Never use cached lists without publication-version invalidation.
        let rows = sqlx::query(
            "SELECT id,title,target_language,author_name,genres FROM books
             WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted
               AND ($1::text IS NULL OR genres @> ARRAY[$1::text])
             ORDER BY id DESC LIMIT $2",
        )
        .bind(genre)
        .bind(i64::try_from(limit).map_err(|_| SearchError::Invalid)?)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
        rows.into_iter()
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
            .collect()
    }
}
