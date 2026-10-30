//! G-06/G-07: durable claims and consumer receipts for the existing shared outbox.
//! This adapter does not dispatch external messages or mutate domain state.
use sqlx::{PgPool, Row};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedEvent {
    pub id: String,
    pub event_type: String,
    pub required_consumers: Vec<String>,
    pub lease_epoch: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxError {
    Invalid,
    LostLease,
    Unavailable,
}

#[derive(Clone)]
pub struct PgOutbox {
    pool: PgPool,
}

impl PgOutbox {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// SKIP LOCKED permits competing workers without duplicate claims.
    /// The lease epoch fences acknowledgments from superseded workers.
    pub async fn claim_next(&self) -> Result<Option<ClaimedEvent>, OutboxError> {
        let row = sqlx::query(
            "WITH next_event AS (
               SELECT id FROM outbox_events
               WHERE delivered_at IS NULL AND dead_at IS NULL
                 AND next_attempt_at<=now()
                 AND (lease_until IS NULL OR lease_until<now())
               ORDER BY next_attempt_at,created_at,id
               FOR UPDATE SKIP LOCKED LIMIT 1
             )
             UPDATE outbox_events AS e
             SET lease_epoch=e.lease_epoch+1,attempts=e.attempts+1,
                 lease_until=now()+interval '60 seconds'
             FROM next_event
             WHERE e.id=next_event.id
             RETURNING e.id::text AS id,e.event_type,
                       e.required_consumers,e.lease_epoch",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        row.map(|row| {
            Ok(ClaimedEvent {
                id: row.try_get("id").map_err(|_| OutboxError::Unavailable)?,
                event_type: row.try_get("event_type").map_err(|_| OutboxError::Unavailable)?,
                required_consumers: row
                    .try_get("required_consumers")
                    .map_err(|_| OutboxError::Unavailable)?,
                lease_epoch: row
                    .try_get("lease_epoch")
                    .map_err(|_| OutboxError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// Records a consumer that has ALREADY completed its idempotent local effect.
    /// An external delivery must use downstream idempotency before calling this.
    /// Completion is set only after every required consumer has a receipt.
    pub async fn acknowledge(
        &self,
        event_id: &str,
        epoch: i64,
        consumer: &str,
    ) -> Result<bool, OutboxError> {
        if epoch <= 0 || event_id.len()!=36 || consumer.is_empty() || consumer.len()>100 {
            return Err(OutboxError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| OutboxError::Unavailable)?;
        let row = sqlx::query(
            "SELECT required_consumers FROM outbox_events
             WHERE id=$1::uuid AND lease_epoch=$2
               AND lease_until>now() AND delivered_at IS NULL AND dead_at IS NULL
             FOR UPDATE",
        )
        .bind(event_id)
        .bind(epoch)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| OutboxError::Invalid)?
        .ok_or(OutboxError::LostLease)?;
        let consumers: Vec<String> = row
            .try_get("required_consumers")
            .map_err(|_| OutboxError::Unavailable)?;
        if !consumers.iter().any(|required| required == consumer) {
            return Err(OutboxError::Invalid);
        }
        sqlx::query(
            "INSERT INTO event_deliveries(consumer,event_id)
             VALUES($1,$2::uuid) ON CONFLICT(consumer,event_id) DO NOTHING",
        )
        .bind(consumer)
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        let missing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM unnest($2::text[]) AS required(consumer)
             WHERE NOT EXISTS (
               SELECT 1 FROM event_deliveries d
               WHERE d.event_id=$1::uuid AND d.consumer=required.consumer
             )",
        )
        .bind(event_id)
        .bind(&consumers)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        if missing == 0 {
            let affected = sqlx::query(
                "UPDATE outbox_events SET delivered_at=now(),lease_until=NULL
                 WHERE id=$1::uuid AND lease_epoch=$2 AND lease_until>now()
                   AND delivered_at IS NULL",
            )
            .bind(event_id)
            .bind(epoch)
            .execute(&mut *tx)
            .await
            .map_err(|_| OutboxError::Unavailable)?;
            if affected.rows_affected()!=1 {
                return Err(OutboxError::LostLease);
            }
        }
        tx.commit().await.map_err(|_| OutboxError::Unavailable)?;
        Ok(missing == 0)
    }
}
