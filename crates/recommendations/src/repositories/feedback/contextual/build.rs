//! One bounded metadata/manual page. No reader or old quality source is queried.
use super::*;
use crate::services::contextual::BUILD_PAGE;
use sha2::{Digest, Sha256};

impl PgRecommendations {
    pub async fn advance_contextual_generation(&self) -> Result<bool, FeedbackError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await?;
        let row=sqlx::query("SELECT id::text,cursor_book_id FROM rec_generations WHERE model_version=$1 AND status='building' ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED")
            .bind(MODEL).fetch_optional(&mut *tx).await?;
        let Some(row) = row else { return Ok(false) };
        let id: String = row.try_get("id")?;
        let cursor: i64 = row.try_get("cursor_book_id")?;
        let books: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM rec_materializable_books WHERE id>$1 ORDER BY id LIMIT $2",
        )
        .bind(cursor)
        .bind(BUILD_PAGE)
        .fetch_all(&mut *tx)
        .await?;
        for book in &books {
            // Already ported NEwBOO content coefficients and the dedicated manual channel.
            // global_sources() is deliberately not called: it reads retired behaviour.
            super::super::sources::book_sources(&mut tx, &id, *book).await?;
        }
        let last = books.last().copied().unwrap_or(cursor);
        if books.len() < BUILD_PAGE as usize {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rec_candidate_edges WHERE generation_id=$1::uuid",
            )
            .bind(&id)
            .fetch_one(&mut *tx)
            .await?;
            // This describes a completed bounded build, not a hash of source book content.
            let manifest: [u8; 32] = Sha256::digest(format!("{MODEL}:{id}:{last}:{count}")).into();
            sqlx::query("UPDATE rec_generations SET cursor_book_id=$2,status='validated',finished_at=now(),manifest_hash=$3 WHERE id=$1::uuid")
                .bind(&id).bind(last).bind(manifest.as_slice()).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE rec_generations SET cursor_book_id=$2 WHERE id=$1::uuid")
                .bind(&id)
                .bind(last)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}
