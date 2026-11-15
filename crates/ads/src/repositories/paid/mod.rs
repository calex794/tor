mod admin;
mod daily_caps;
mod dashboard;
mod events;
mod planner;
mod ports;
mod retention;
mod serving;
use crate::domain::paid::*;
use sqlx::{PgPool, Postgres, Transaction};
use tor_contracts::ads::{AdActor, AdError};

#[derive(Clone)]
pub struct PgPaidAds {
    pub(super) pool: PgPool,
}
impl PgPaidAds {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
pub(super) fn db(e: sqlx::Error) -> AdError {
    match e.as_database_error().and_then(|e| e.code()).as_deref() {
        Some("23505") => AdError::Conflict,
        Some("23503") => AdError::NotFound,
        Some("23514" | "22003" | "22P02") => AdError::Invalid,
        _ => AdError::Unavailable,
    }
}
pub(super) async fn limits(tx: &mut Transaction<'_, Postgres>) -> Result<(), AdError> {
    sqlx::query("SET LOCAL statement_timeout='750ms'")
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}
pub(super) async fn actor(
    tx: &mut Transaction<'_, Postgres>,
    a: &AdActor,
    admin: bool,
) -> Result<(), AdError> {
    let verified: Option<i64> = sqlx::query_scalar(
        "SELECT u.id FROM users u JOIN sessions s ON s.user_id=u.id
      WHERE u.id=$1 AND u.account_state='active' AND s.token_hash=$2
        AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() FOR SHARE OF u,s",
    )
    .bind(a.user_id)
    .bind(a.session_digest.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    if verified.is_none() {
        return Err(AdError::Denied);
    }
    if admin {
        let role: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM account_roles
        WHERE user_id=$1 AND role='administrator' AND is_active FOR SHARE",
        )
        .bind(a.user_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
        if role.is_none() {
            return Err(AdError::Denied);
        }
    }
    Ok(())
}
