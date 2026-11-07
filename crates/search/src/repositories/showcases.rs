//! F-04: bounded public-only PostgreSQL projection for homepage/genre spotlights.
use crate::{
    domain::{SearchBook, SearchError},
    services::showcases::{ReleaseShowcaseRepository, ShowcaseRepository},
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

fn decode_book(row: sqlx::postgres::PgRow) -> Result<SearchBook, SearchError> {
    let id: i64 = row.try_get("id").map_err(|_| SearchError::Unavailable)?;
    Ok(SearchBook {
        id: u64::try_from(id).map_err(|_| SearchError::Unavailable)?,
        title: row.try_get("title").map_err(|_| SearchError::Unavailable)?,
        language: row.try_get("target_language").map_err(|_| SearchError::Unavailable)?,
        author_name: row.try_get("author_name").map_err(|_| SearchError::Unavailable)?,
        genres: row.try_get("genres").map_err(|_| SearchError::Unavailable)?,
        tags: row.try_get("tags").map_err(|_| SearchError::Unavailable)?,
        fandoms: row.try_get("fandoms").map_err(|_| SearchError::Unavailable)?,
    })
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
            "SELECT id,title,target_language,author_name,genres,tags,fandoms FROM books
             WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND lifecycle_status<>'pending_deletion' AND NOT admin_hold AND NOT is_deleted
               AND ($1::text IS NULL OR genres @> ARRAY[$1::text])
             ORDER BY id DESC LIMIT $2",
        )
        .bind(genre)
        .bind(i64::try_from(limit).map_err(|_| SearchError::Invalid)?)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
        rows.into_iter().map(decode_book).collect()
    }
}

#[async_trait]
impl ReleaseShowcaseRepository for PgShowcases {
    async fn recent_free_releases(&self, limit: usize) -> Result<Vec<SearchBook>, SearchError> {
        if !(1..=24).contains(&limit) {
            return Err(SearchError::Invalid);
        }
        // B owns immutable chapter_first_release_events. Editing an existing
        // chapter does not promote it, and a revoked publication disappears.
        // This is not a notification queue, popularity score or insertion order.
        let rows = sqlx::query(
            "SELECT b.id,b.title,b.target_language,b.author_name,b.genres,b.tags,b.fandoms
             FROM books b
             JOIN chapters c ON c.book_id=b.id
             JOIN chapter_first_release_events e ON e.chapter_id=c.id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
             WHERE b.visibility='public' AND b.visibility_status='active'
               AND b.access_level='basic'
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='public' AND c.access_level='free'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
             GROUP BY b.id
             ORDER BY MAX(e.created_at) DESC,b.id DESC
             LIMIT $1",
        )
        .bind(i64::try_from(limit).map_err(|_| SearchError::Invalid)?)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| SearchError::Unavailable)?;
        rows.into_iter().map(decode_book).collect()
    }
}
