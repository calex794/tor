//! REC-13 PG adapter. Explicit settings and optional analytics have separate lifecycles.
use crate::domain::feedback::FeedbackError;
use sqlx::{PgPool, Postgres, Transaction};

mod assignments;
mod consent;
mod contextual;
mod conversions;
pub(crate) mod discovery;
mod flow_consent;
mod generations;
mod maintenance;
mod observations;
mod organic;
mod preference_retention;
mod preferences;
mod retirement;
mod serving;
mod shelf;
mod shelf_policy;
pub use shelf_policy::{ShelfShareAuditRow, ShelfShareSnapshot};
mod sources;
mod status;

// Public preference CAS versions include a retention epoch in their high digits.
pub(super) const REVISION_STRIDE: i64 = 1_000_000_000;

#[derive(Clone)]
pub struct PgRecommendations {
    pub(super) pool: PgPool,
}
impl PgRecommendations {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn probe(&self) -> Result<(), FeedbackError> {
        sqlx::query_scalar::<_, bool>("SELECT singleton FROM rec_runtime_policy WHERE singleton")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }
}
impl From<sqlx::Error> for FeedbackError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
// All explicit commands share one namespace, even while the legacy consent
// controls still exist. Lock A before the preference head, then an optional
// analytics subject; account deletion takes the same leading A lock.
async fn lock_preferences(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
) -> Result<i64, FeedbackError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE",
    )
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FeedbackError::Forbidden)?;
    sqlx::query("INSERT INTO rec_preference_accounts(user_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(actor)
        .execute(&mut **tx)
        .await?;
    Ok(sqlx::query_scalar(
        "SELECT feedback_epoch FROM rec_preference_accounts WHERE user_id=$1 FOR UPDATE",
    )
    .bind(actor)
    .fetch_one(&mut **tx)
    .await?)
}

// A key expires with its 30-day receipt. Removing only the requested expired
// key under the common head lock gives the same behavior as retention. A later
// CAS/ACL failure rolls back this deletion together with the rejected command.
async fn discard_expired_command(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    key: &[u8; 32],
) -> Result<(), FeedbackError> {
    sqlx::query(
        "DELETE FROM rec_user_preference_commands
         WHERE user_id=$1 AND command_key=$2
           AND created_at<=now()-interval '30 days'",
    )
    .bind(actor)
    .bind(key.as_slice())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn lock_subject(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
) -> Result<String, FeedbackError> {
    sqlx::query_scalar(
        "SELECT s.subject_id::text FROM rec_subjects s
         JOIN users u ON u.id=s.user_id AND u.account_state='active'
         WHERE s.user_id=$1 AND s.enabled FOR UPDATE OF s FOR SHARE OF u",
    )
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FeedbackError::Forbidden)
}
async fn dirty(tx: &mut Transaction<'_, Postgres>, assignment: &str) -> Result<(), FeedbackError> {
    sqlx::query("INSERT INTO rec_dirty_buckets(bucket_date,book_id) SELECT cohort_day,book_id FROM rec_assignments WHERE id=$1::uuid ON CONFLICT DO NOTHING")
        .bind(assignment).execute(&mut **tx).await?;
    Ok(())
}
