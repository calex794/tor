//! G-09: immutable log of administrator commands for read-only S3 reconciliation.
//! This repository records requests only; it never deletes or downloads objects.
use sqlx::PgPool;

use crate::{repositories::reconciliation::InventoryError, services::reconciliation::PrivateScope};

#[derive(Clone)]
pub struct PgReconciliationCommands {
    pool: PgPool,
}

impl PgReconciliationCommands {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fail-closed: if an action cannot be audited, it must not run.
    /// Actor identity is derived by the HTTP layer from the current live session.
    pub async fn record(
        &self,
        actor_id: i64,
        direction: &'static str,
        scope: PrivateScope,
        action: &'static str,
    ) -> Result<(), InventoryError> {
        if actor_id <= 0
            || !matches!(direction, "s3_to_pg" | "pg_to_s3")
            || !matches!(action, "step" | "next_cycle" | "resume_exhausted")
            || (action == "resume_exhausted" && direction != "pg_to_s3")
        {
            return Err(InventoryError::Invalid);
        }
        sqlx::query(
            "INSERT INTO g09_operator_requests(actor_user_id,direction,scope,action)
             VALUES ($1,$2,$3,$4)",
        )
        .bind(actor_id)
        .bind(direction)
        .bind(scope.kind())
        .bind(action)
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        Ok(())
    }
}
