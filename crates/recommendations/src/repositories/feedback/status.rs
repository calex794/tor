use super::*;
use sqlx::Row;

impl PgRecommendations {
    pub async fn recent_generations(&self) -> Result<Vec<(String, String, i64)>, FeedbackError> {
        let rows=sqlx::query("SELECT g.id::text,g.status,(SELECT count(*) FROM rec_candidate_edges e WHERE e.generation_id=g.id)::bigint AS edges FROM rec_generations g ORDER BY g.created_at DESC,g.id LIMIT 10")
            .fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| Ok((r.try_get("id")?, r.try_get("status")?, r.try_get("edges")?)))
            .collect()
    }
    /// A public book ID only; no chapter text, rights or storage metadata.
    pub async fn seed_for_chapter(&self, chapter: i64) -> Result<Option<i64>, FeedbackError> {
        if chapter <= 0 {
            return Err(FeedbackError::Invalid);
        }
        Ok(
            sqlx::query_scalar("SELECT book_id FROM rec_public_chapter_links WHERE chapter_id=$1")
                .bind(chapter)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    /// One background refresh per hour at most, retaining the last active generation.
    pub async fn schedule_due_refresh(&self) -> Result<(), FeedbackError> {
        let due:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM rec_generations WHERE status='building' OR created_at>now()-interval '1 hour')")
            .fetch_one(&self.pool).await?;
        if due {
            self.request_refresh().await?;
        }
        Ok(())
    }
}
