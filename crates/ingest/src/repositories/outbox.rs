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

/// Failure is applied only by the holder of the live, fenced lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDisposition {
    RetryScheduled,
    DeadLetter,
}

#[derive(Clone)]
pub struct PgOutbox {
    pool: PgPool,
}

impl PgOutbox {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A worker may crash on its final lease without recording an explicit
    /// failure. Reap such events in bounded batches, rather than reclaiming
    /// them indefinitely. A live lease is never reaped.
    pub async fn reap_exhausted(&self) -> Result<u64, OutboxError> {
        let result = sqlx::query(
            "WITH exhausted AS (
               SELECT id FROM outbox_events
               WHERE delivered_at IS NULL AND dead_at IS NULL
                 AND attempts>=12 AND next_attempt_at<=now()
                 AND (lease_until IS NULL OR lease_until<now())
               ORDER BY next_attempt_at,created_at,id
               FOR UPDATE SKIP LOCKED LIMIT 64
             )
             UPDATE outbox_events AS event
             SET dead_at=now(),lease_until=NULL
             FROM exhausted WHERE event.id=exhausted.id",
        )
        .execute(&self.pool)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        Ok(result.rows_affected())
    }

    /// Claim only work owned by this consumer. Unlike a generic outbox claim,
    /// this never leases unrelated payments, search projections or work whose
    /// consumer receipt was already committed.
    pub async fn claim_next_for_consumer(
        &self,
        event_type: &str,
        consumer: &str,
    ) -> Result<Option<ClaimedEvent>, OutboxError> {
        if event_type.is_empty()
            || event_type.len() > 100
            || consumer.is_empty()
            || consumer.len() > 100
        {
            return Err(OutboxError::Invalid);
        }
        self.reap_exhausted().await?;
        let row = sqlx::query(
            "WITH due AS (
               SELECT e.id FROM outbox_events AS e
               WHERE e.event_type=$1
                 AND $2=ANY(e.required_consumers)
                 AND NOT EXISTS (
                   SELECT 1 FROM event_deliveries AS delivery
                   WHERE delivery.event_id=e.id AND delivery.consumer=$2
                 )
                 AND e.delivered_at IS NULL AND e.dead_at IS NULL
                 AND e.attempts<12 AND e.next_attempt_at<=now()
                 AND (e.lease_until IS NULL OR e.lease_until<now())
               ORDER BY e.next_attempt_at,e.created_at,e.id
               FOR UPDATE OF e SKIP LOCKED LIMIT 1
             )
             UPDATE outbox_events AS event
             SET lease_epoch=event.lease_epoch+1,
                 attempts=event.attempts+1,
                 lease_until=now()+interval '60 seconds'
             FROM due WHERE event.id=due.id
             RETURNING event.id::text AS id,event.event_type,
                       event.required_consumers,event.lease_epoch",
        )
        .bind(event_type)
        .bind(consumer)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        row.map(|row| {
            Ok(ClaimedEvent {
                id: row.try_get("id").map_err(|_| OutboxError::Unavailable)?,
                event_type: row
                    .try_get("event_type")
                    .map_err(|_| OutboxError::Unavailable)?,
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

    /// SKIP LOCKED permits competing workers without duplicate claims.
    /// The lease epoch fences acknowledgments from superseded workers.
    pub async fn claim_next(&self) -> Result<Option<ClaimedEvent>, OutboxError> {
        self.reap_exhausted().await?;
        let row = sqlx::query(
            "WITH next_event AS (
               SELECT id FROM outbox_events
               WHERE delivered_at IS NULL AND dead_at IS NULL
                 AND attempts<12 AND next_attempt_at<=now()
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
                event_type: row
                    .try_get("event_type")
                    .map_err(|_| OutboxError::Unavailable)?,
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
        if epoch <= 0 || event_id.len() != 36 || consumer.is_empty() || consumer.len() > 100 {
            return Err(OutboxError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| OutboxError::Unavailable)?;
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
            if affected.rows_affected() != 1 {
                return Err(OutboxError::LostLease);
            }
        }
        tx.commit().await.map_err(|_| OutboxError::Unavailable)?;
        Ok(missing == 0)
    }

    /// Release a failed lease, preserving consumer receipts already committed.
    /// Retry delay is exponential (5s..1h), with bounded jitter to avoid a
    /// thundering herd. After 12 claims, or on a non-retryable failure, the
    /// event is dead-lettered. This never runs external side effects.
    pub async fn record_failure(
        &self,
        event_id: &str,
        epoch: i64,
        retryable: bool,
    ) -> Result<FailureDisposition, OutboxError> {
        if epoch <= 0 || event_id.len() != 36 {
            return Err(OutboxError::Invalid);
        }
        let dead: Option<bool> = sqlx::query_scalar(
            "UPDATE outbox_events
             SET lease_until=NULL,
                 next_attempt_at=CASE
                   WHEN $3 AND attempts<12 THEN
                     now()+make_interval(secs=>LEAST(3600,
                       5*(1 << LEAST(attempts-1,10))
                       +floor(random()*5)::integer))
                   ELSE next_attempt_at
                 END,
                 dead_at=CASE
                   WHEN NOT $3 OR attempts>=12 THEN now()
                   ELSE dead_at
                 END
             WHERE id=$1::uuid AND lease_epoch=$2 AND lease_until>now()
               AND delivered_at IS NULL AND dead_at IS NULL
             RETURNING dead_at IS NOT NULL",
        )
        .bind(event_id)
        .bind(epoch)
        .bind(retryable)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        match dead {
            Some(true) => Ok(FailureDisposition::DeadLetter),
            Some(false) => Ok(FailureDisposition::RetryScheduled),
            None => Err(OutboxError::LostLease),
        }
    }
}
