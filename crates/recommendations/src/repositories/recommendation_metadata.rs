//! REC-10: one bounded live B-owned books lookup, never historical cache metadata.
//! This adapter only applies the canonical B public/hold/deletion predicate.
//! Actor-specific age and access policy must be added by I before HTTP exposure.
use sqlx::{PgPool, Row};

use crate::services::assembly::{
    assemble_visible, requested_book_ids, AssemblyError, PublicRecommendation, VisibleBookMetadata,
};
use crate::services::hybrid_scoring::ScoredCandidate;

#[derive(Clone)]
pub struct PgRecommendationMetadata {
    pool: PgPool,
}

impl PgRecommendationMetadata {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A single SQL statement checks ALL scored IDs against current B state.
    /// It fetches only public book metadata and performs no N+1, paid,
    /// profile, chapter or raw-reading-event lookups.
    pub async fn assemble_public(
        &self,
        scored: &[ScoredCandidate],
        limit: usize,
        exclude_book_id: Option<i64>,
    ) -> Result<Vec<PublicRecommendation>, AssemblyError> {
        let ids = requested_book_ids(scored, limit, exclude_book_id)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT id,title,target_language,author_name
             FROM books
             WHERE id = ANY($1::bigint[])
               AND visibility = 'public'
               AND NOT admin_hold AND NOT is_deleted
             ORDER BY id",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AssemblyError::Unavailable)?;
        let mut visible = Vec::with_capacity(rows.len());
        for row in rows {
            visible.push(VisibleBookMetadata {
                book_id: row.try_get("id").map_err(|_| AssemblyError::Unavailable)?,
                title: row.try_get("title").map_err(|_| AssemblyError::Unavailable)?,
                author_name: row
                    .try_get("author_name")
                    .map_err(|_| AssemblyError::Unavailable)?,
                language: row
                    .try_get("target_language")
                    .map_err(|_| AssemblyError::Unavailable)?,
            });
        }
        assemble_visible(scored, visible, limit, exclude_book_id)
    }
}
