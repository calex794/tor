//! REC-13 PG adapter. All mutations serialize with consent/erasure.
use sqlx::{PgPool, Postgres, Transaction};
use crate::domain::feedback::FeedbackError;

mod consent;
mod preferences;
mod assignments;
mod observations;
mod conversions;
mod maintenance;
mod sources;
mod generations;

#[derive(Clone)]
pub struct PgRecommendations {
    pub(super) pool: PgPool,
}
impl PgRecommendations {
    pub fn new(pool: PgPool) -> Self { Self { pool } }
    pub async fn probe(&self) -> Result<(), FeedbackError> {
        sqlx::query_scalar::<_, bool>("SELECT singleton FROM rec_runtime_policy WHERE singleton")
            .fetch_one(&self.pool).await?;
        Ok(())
    }
}
impl From<sqlx::Error> for FeedbackError {
    fn from(_: sqlx::Error) -> Self { Self::Unavailable }
}
async fn lock_subject(tx: &mut Transaction<'_, Postgres>, actor: i64) -> Result<String, FeedbackError> {
    sqlx::query_scalar("SELECT subject_id::text FROM rec_subjects WHERE user_id=$1 AND enabled FOR UPDATE")
        .bind(actor).fetch_optional(&mut **tx).await?.ok_or(FeedbackError::Forbidden)
}
async fn dirty(tx: &mut Transaction<'_, Postgres>, assignment: &str) -> Result<(), FeedbackError> {
    sqlx::query("INSERT INTO rec_dirty_buckets(bucket_date,book_id) SELECT cohort_day,book_id FROM rec_assignments WHERE id=$1::uuid ON CONFLICT DO NOTHING")
        .bind(assignment).execute(&mut **tx).await?;
    Ok(())
}
