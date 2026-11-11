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

    /// Operator maintenance only. Consumers claim narrowly to avoid touching
    /// another subsystem's events and receipts.
    pub async fn reap_exhausted(&self) -> Result<u64, OutboxError> {
        let result = sqlx::query(
            "WITH exhausted AS (
               SELECT id FROM outbox_events
               WHERE delivered_at IS NULL AND dead_at IS NULL
                 AND attempts>=12 AND next_attempt_at<=clock_timestamp()
                 AND (lease_until IS NULL OR lease_until<clock_timestamp())
               ORDER BY next_attempt_at,created_at,id
               FOR UPDATE SKIP LOCKED LIMIT 64
             )
             UPDATE outbox_events AS event
             SET dead_at=clock_timestamp(),lease_until=NULL
             FROM exhausted WHERE event.id=exhausted.id",
        )
        .execute(&self.pool)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        Ok(result.rows_affected())
    }

    /// Reap only the selected event family; never change unrelated outboxes.
    async fn reap_exhausted_for(
        &self,
        event_type: &str,
        consumer: &str,
    ) -> Result<(), OutboxError> {
        sqlx::query(
            "WITH exhausted AS (
               SELECT e.id FROM outbox_events e
               WHERE e.event_type=$1
                 AND e.required_consumers @> ARRAY[$2::text]
                 AND NOT EXISTS (
                     SELECT 1 FROM event_deliveries d
                     WHERE d.event_id=e.id AND d.consumer=$2
                 )
                 AND e.delivered_at IS NULL AND e.dead_at IS NULL
                 AND e.attempts>=12 AND e.next_attempt_at<=clock_timestamp()
                 AND (e.lease_until IS NULL OR e.lease_until<clock_timestamp())
               ORDER BY e.next_attempt_at,e.created_at,e.id
               FOR UPDATE SKIP LOCKED LIMIT 64
             )
             UPDATE outbox_events AS e SET dead_at=clock_timestamp(),lease_until=NULL
             FROM exhausted WHERE e.id=exhausted.id",
        )
        .bind(event_type)
        .bind(consumer)
        .execute(&self.pool)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        Ok(())
    }

    /// A claimant is allowed to handle only its own event type and consumer.
    /// Previously received events are not leased again. This API never starts
    /// a worker or sends a notification.
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
        self.reap_exhausted_for(event_type, consumer).await?;
        let row = sqlx::query(
            "WITH next_event AS (
               SELECT e.id FROM outbox_events e
               WHERE e.event_type=$1
                 AND e.required_consumers @> ARRAY[$2::text]
                 AND NOT EXISTS (
                     SELECT 1 FROM event_deliveries d
                     WHERE d.event_id=e.id AND d.consumer=$2
                 )
                 AND e.delivered_at IS NULL AND e.dead_at IS NULL
                 AND e.attempts<12 AND e.next_attempt_at<=clock_timestamp()
                 AND (e.lease_until IS NULL OR e.lease_until<clock_timestamp())
               ORDER BY e.next_attempt_at,e.created_at,e.id
               FOR UPDATE SKIP LOCKED LIMIT 1
             )
             UPDATE outbox_events AS e
             SET lease_epoch=e.lease_epoch+1,attempts=e.attempts+1,
                 lease_until=clock_timestamp()+interval '60 seconds'
             FROM next_event
             WHERE e.id=next_event.id
             RETURNING e.id::text AS id,e.event_type,
                       e.required_consumers,e.lease_epoch",
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

    /// Extend only a still-active generation. An expired worker cannot
    /// resurrect its lease or race a successor's acknowledgement.
    pub async fn renew_lease(&self, event_id: &str, epoch: i64) -> Result<(), OutboxError> {
        if event_id.len() != 36 || epoch <= 0 {
            return Err(OutboxError::Invalid);
        }
        let row = sqlx::query_scalar::<_, i64>(
            "UPDATE outbox_events SET lease_until=clock_timestamp()+interval '60 seconds'
             WHERE id=$1::uuid AND lease_epoch=$2 AND lease_until>clock_timestamp()
               AND delivered_at IS NULL AND dead_at IS NULL
             RETURNING lease_epoch",
        )
        .bind(event_id)
        .bind(epoch)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        if row.is_none() {
            return Err(OutboxError::LostLease);
        }
        Ok(())
    }

    /// Hand an unfinished event to its next required consumer without waiting
    /// for TTL. Existing receipts stay durable; only the current generation
    /// can release a lease, and a released one cannot acknowledge anything.
    pub async fn release_unfinished(&self, event_id: &str, epoch: i64) -> Result<(), OutboxError> {
        if event_id.len() != 36 || epoch <= 0 {
            return Err(OutboxError::Invalid);
        }
        let row = sqlx::query_scalar::<_, i64>(
            "UPDATE outbox_events SET lease_until=NULL
             WHERE id=$1::uuid AND lease_epoch=$2 AND lease_until>clock_timestamp()
               AND delivered_at IS NULL AND dead_at IS NULL
             RETURNING lease_epoch",
        )
        .bind(event_id)
        .bind(epoch)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| OutboxError::Unavailable)?;
        if row.is_none() {
            return Err(OutboxError::LostLease);
        }
        Ok(())
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
