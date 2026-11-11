//! Atomic editorial state. B-owned discovery views remain the metadata authority.
mod commands;
mod report;
mod serving;
use crate::domain::editorial::EditorialError;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgEditorial {
    pool: PgPool,
}
impl PgEditorial {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
impl From<sqlx::Error> for EditorialError {
    fn from(value: sqlx::Error) -> Self {
        match value.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("42501") => Self::Denied,
            Some("23503") => Self::NotFound,
            Some("23505") => Self::Conflict,
            Some("23514") => Self::Invalid,
            _ => Self::Unavailable,
        }
    }
}
async fn limits(tx: &mut Transaction<'_, Postgres>, read: bool) -> Result<(), EditorialError> {
    if read {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("SET LOCAL statement_timeout='750ms'")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='75ms'")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
