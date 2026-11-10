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
        sqlx::query("SELECT 1 FROM g09_private_inventory_scans LIMIT 0")
            .execute(&self.pool)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        sqlx::query("SELECT 1 FROM g09_registered_object_scans LIMIT 0")
            .execute(&self.pool)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        Ok(())
    }

    /// PostgreSQL checkpoint provides cross-replica rate control. An
    /// unfinished cycle advances at most once per fifteen minutes per
    /// scope/direction. Only finished cycles may restart, >= 24 hours after
    /// their last checkpoint. An exhausted reverse scan is NEVER resumed.
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
