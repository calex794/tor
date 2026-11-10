//! Consent-scoped F read model over authoritative C ports, never a second outbox.
use crate::services::flow_funnel::FlowFunnelError;
use sqlx::{PgPool, Postgres, Transaction};

mod actions;
mod binding;
mod capture;
mod events;
mod maintenance;
mod reconciliation;
mod report;

const CONSUMER: &str = "flow-read-attribution-v1";

#[derive(Clone)]
pub struct PgFlowFunnel {
    pool: PgPool,
}

impl PgFlowFunnel {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Record the last worker's local mode. This is diagnostic information,
    /// never permission to attribute a command handled by another process.
    pub async fn initialize(&self, collect: bool) -> Result<(), FlowFunnelError> {
        let mut tx = self.begin().await?;
        sqlx::query(
            "INSERT INTO flow_book_worker_health(singleton,last_collecting,collecting_since,last_tick,backlog)
             VALUES(true,$1,CASE WHEN $1 THEN clock_timestamp() ELSE NULL END,clock_timestamp(),true)
             ON CONFLICT(singleton) DO UPDATE SET
               collecting_since=CASE WHEN EXCLUDED.last_collecting THEN
                 CASE WHEN flow_book_worker_health.last_collecting
                      THEN flow_book_worker_health.collecting_since ELSE clock_timestamp() END
                 ELSE NULL END,
               last_collecting=EXCLUDED.last_collecting,last_tick=clock_timestamp(),backlog=true",
        )
        .bind(collect)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }

    /// Durable receipts and each derivative update commit together. Maintenance
    /// is unconditional; a stopped collector must not stop privacy expiry.
    pub async fn cycle(&self, collect: bool) -> Result<u64, FlowFunnelError> {
        self.initialize(collect).await?;
        let mut affected = 0;
        if collect {
            affected += self.capture_books().await?;
            for event in self.pending_reads().await? {
                affected += u64::from(self.accept_read_event(&event, true).await?);
            }
            affected += self.capture_actions().await?;
        }
        affected += self.maintain().await?;
        let mut tx = self.begin().await?;
        let backlog: bool = sqlx::query_scalar(BACKLOG_SQL)
            .bind(CONSUMER)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        let backlog = backlog || maintenance::pending(&mut tx).await?;
        sqlx::query(
            "UPDATE flow_book_worker_health SET backlog=$1,
               last_success=CASE WHEN $1 THEN last_success ELSE clock_timestamp() END
             WHERE singleton",
        )
        .bind(backlog)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(affected)
    }

    async fn begin(&self) -> Result<Transaction<'_, Postgres>, FlowFunnelError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        Ok(tx)
    }
}

fn unavailable(_: sqlx::Error) -> FlowFunnelError {
    FlowFunnelError::Unavailable
}

const BACKLOG_SQL: &str = r#"
SELECT EXISTS(SELECT 1 FROM flow_visit_receipts
       WHERE book_id IS NOT NULL AND NOT funnel_processed
         AND observed_at<=clock_timestamp()-interval '305 seconds')
 OR EXISTS(SELECT 1 FROM reader_flow_verified_read_events v
       JOIN reader_flow_verified_read_receipts r ON r.event_id=v.event_id AND r.occurred_at=v.occurred_at
       JOIN rec_subjects s ON s.subject_id=r.subject_id AND s.flow_epoch=r.flow_epoch
         AND s.user_id=v.user_id AND s.enabled
       JOIN users u ON u.id=s.user_id AND u.account_state='active'
       WHERE v.occurred_at>clock_timestamp()-interval '30 days'
         AND v.occurred_at<=clock_timestamp()-interval '305 seconds'
         AND v.occurred_at>=s.flow_enabled_at
         AND NOT EXISTS(SELECT 1 FROM event_deliveries d WHERE d.consumer=$1 AND d.event_id=v.event_id))
 OR EXISTS(SELECT 1 FROM reader_flow_verified_book_actions a
       WHERE occurred_at<=clock_timestamp()-interval '305 seconds'
         AND occurred_at>clock_timestamp()-interval '30 days'
         AND NOT EXISTS(SELECT 1 FROM flow_book_action_receipts r WHERE r.event_id=a.event_id))
 OR EXISTS(SELECT 1 FROM flow_book_cohorts WHERE expires_at<=clock_timestamp())
 OR EXISTS(SELECT 1 FROM flow_book_action_receipts WHERE occurred_at<clock_timestamp()-interval '30 days')
 OR EXISTS(SELECT 1 FROM flow_book_daily WHERE cohort_day<(clock_timestamp() AT TIME ZONE 'UTC')::date-179)
 OR EXISTS(SELECT 1 FROM event_deliveries d WHERE d.consumer=$1 AND NOT EXISTS(
       SELECT 1 FROM reader_flow_verified_read_receipts r WHERE r.event_id=d.event_id
         AND r.occurred_at>clock_timestamp()-interval '30 days'))
"#;
