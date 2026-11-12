//! G-09 read-only scheduling ledger. A tick chooses at most one LIST/HEAD
//! page per private scope and direction. Never automatically GETs ciphertext,
//! resumes exhausted retries, deletes a key, or changes reader entitlements.
use crate::{
    domain::provider::valid_private_bucket, repositories::reconciliation::InventoryError,
    services::reconciliation::PrivateScope,
};
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduledAuditAction {
    Wait,
    ScanPage,
    NewCycle,
}

fn action_for(completed: bool, exhausted: bool, due: bool) -> ScheduledAuditAction {
    if exhausted || !due {
        ScheduledAuditAction::Wait
    } else if completed {
        ScheduledAuditAction::NewCycle
    } else {
        ScheduledAuditAction::ScanPage
    }
}

#[derive(Clone)]
pub struct PgG09Schedule {
    pool: PgPool,
}

impl PgG09Schedule {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fail startup for an explicitly enabled scheduler if its diagnostic
    /// migrations are absent; never try to migrate production automatically.
    pub async fn probe(&self) -> Result<(), InventoryError> {
        sqlx::query(
            "SELECT exhausted,unknown_attempts,retries FROM g09_private_inventory_scans LIMIT 0",
        )
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        sqlx::query("SELECT 1 FROM g09_registered_object_scans LIMIT 0")
            .execute(&self.pool)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        sqlx::query("SELECT next_allowed_at FROM g09_scheduled_pages LIMIT 0")
            .execute(&self.pool)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        Ok(())
    }

    /// Reserve the global rate slot *atomically* across TOR replicas.
    /// Skipping a slot after an outage is deliberate: no catch-up bursts.
    pub async fn reserve_slot(
        &self,
        bucket: &str,
        scope: PrivateScope,
        reverse: bool,
    ) -> Result<bool, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let direction = if reverse { "pg-to-s3" } else { "s3-to-pg" };
        let claimed: Option<bool> = sqlx::query_scalar(
            "INSERT INTO g09_scheduled_pages(bucket,scope,direction,next_allowed_at)
             VALUES($1,$2,$3,clock_timestamp()+interval '15 minutes')
             ON CONFLICT (bucket,scope,direction) DO UPDATE
                SET next_allowed_at=clock_timestamp()+interval '15 minutes'
              WHERE g09_scheduled_pages.next_allowed_at<=clock_timestamp()
             RETURNING TRUE",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(direction)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        Ok(claimed == Some(true))
    }

    /// Checkpoint age is a separate eligibility check, not an atomic lock.
    /// The worker MUST also call reserve_slot() before running any page.
    /// Unfinished cycles are due every 15 minutes; completed cycles may
    /// restart no earlier than 24 hours later. Exhausted scans stay stopped.
    pub async fn action(
        &self,
        bucket: &str,
        scope: PrivateScope,
        reverse: bool,
    ) -> Result<ScheduledAuditAction, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let sql = if reverse {
            "SELECT completed,exhausted,
                    CASE WHEN completed THEN
                      coalesce(checkpoint_at <= clock_timestamp()-interval '24 hours',false)
                    ELSE
                      coalesce(checkpoint_at <= clock_timestamp()-interval '15 minutes',true)
                    END AS due
               FROM g09_registered_object_scans
              WHERE bucket=$1 AND scope=$2"
        } else {
            "SELECT completed,exhausted,
                    CASE WHEN completed THEN
                      coalesce(checkpoint_at <= clock_timestamp()-interval '24 hours',false)
                    ELSE
                      coalesce(checkpoint_at <= clock_timestamp()-interval '15 minutes',true)
                    END AS due
               FROM g09_private_inventory_scans
              WHERE bucket=$1 AND scope=$2"
        };
        let row: Option<(bool, bool, bool)> = sqlx::query_as(sql)
            .bind(bucket)
            .bind(scope.kind())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        Ok(row.map_or(
            ScheduledAuditAction::ScanPage,
            |(completed, exhausted, due)| action_for(completed, exhausted, due),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{action_for, ScheduledAuditAction as A};

    #[test]
    fn never_resume_exhausted_or_repeat_recent_checkpoint() {
        assert_eq!(action_for(false, true, true), A::Wait);
        assert_eq!(action_for(true, true, true), A::Wait);
        assert_eq!(action_for(false, false, false), A::Wait);
        assert_eq!(action_for(true, false, false), A::Wait);
        assert_eq!(action_for(false, false, true), A::ScanPage);
        assert_eq!(action_for(true, false, true), A::NewCycle);
    }
}
