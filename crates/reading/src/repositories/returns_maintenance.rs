//! Supervised heartbeat, bounded finalization, and privacy-window cleanup.
use super::{unavailable, PgReadingReturns, ReadingReturnsError};
use sqlx::{Postgres, Transaction};
use std::time::{Duration, Instant};

impl PgReadingReturns {
    /// Call once when the collector process starts, including disabled starts.
    /// A fast restart is a coverage boundary even inside the heartbeat grace.
    pub async fn start_collection(&self, enabled: bool) -> Result<(), ReadingReturnsError> {
        self.supervised_tick(enabled, true).await
    }

    /// The G/I worker calls this every five seconds. Cleanup runs while disabled.
    pub async fn tick_collection(&self, enabled: bool) -> Result<(), ReadingReturnsError> {
        self.supervised_tick(enabled, false).await
    }

    async fn supervised_tick(
        &self,
        enabled: bool,
        restart: bool,
    ) -> Result<(), ReadingReturnsError> {
        match self.cycle(enabled, restart).await {
            Ok(()) => Ok(()),
            Err(error) => {
                // The failed cycle rolled back its unpublished heartbeat.
                // A maintenance failure is a real break, not a healthy worker.
                sqlx::query("UPDATE reader_return_coverage SET ended_at=last_heartbeat_at WHERE ended_at IS NULL")
                    .execute(&self.pool).await.map_err(unavailable)?;
                Err(error)
            }
        }
    }

    async fn cycle(&self, enabled: bool, restart: bool) -> Result<(), ReadingReturnsError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('reader-return-coverage-v1',0))",
        )
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query(
            "UPDATE reader_return_coverage SET ended_at=last_heartbeat_at
             WHERE ended_at IS NULL AND ($1 OR NOT $2
               OR last_heartbeat_at<clock_timestamp()-interval '60 seconds')",
        )
        .bind(restart)
        .bind(enabled)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        if enabled {
            sqlx::query(
                "WITH observed AS MATERIALIZED (SELECT clock_timestamp() AS at)
                 INSERT INTO reader_return_coverage(started_at,last_heartbeat_at)
                 SELECT at,at FROM observed
                 ON CONFLICT ((true)) WHERE ended_at IS NULL
                 DO UPDATE SET last_heartbeat_at=EXCLUDED.last_heartbeat_at",
            )
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        }
        // Publish heartbeat and privacy cleanup together. Finalization sees
        // this fresh heartbeat even on the first tick just after midnight.
        // Otherwise a healthy D30 would be frozen as unknown one tick early.
        if !maintain(&mut tx).await? {
            sqlx::query("UPDATE reader_return_coverage SET ended_at=last_heartbeat_at WHERE ended_at IS NULL")
                .execute(&mut *tx).await.map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)
    }
}

async fn maintain(tx: &mut Transaction<'_, Postgres>) -> Result<bool, ReadingReturnsError> {
    let started = Instant::now();
    // A normal midnight cohort can exceed one 500-row batch. Finish a bounded
    // backlog in this transaction instead of manufacturing a five-second gap
    // between otherwise healthy ticks. The time budget is checked between
    // batches; it does not cancel a partially completed SQL statement.
    for _ in 0..16 {
        if maintain_batch(tx).await? {
            return Ok(true);
        }
        if started.elapsed() >= Duration::from_millis(500) {
            break;
        }
    }
    Ok(false)
}

async fn maintain_batch(tx: &mut Transaction<'_, Postgres>) -> Result<bool, ReadingReturnsError> {
    sqlx::query(FINALIZE)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    for statement in [
            "DELETE FROM reader_activity_days WHERE (subject_id,book_id,activity_day) IN (
               SELECT subject_id,book_id,activity_day FROM reader_activity_days
               WHERE first_seen_at<clock_timestamp()-interval '30 days'
               ORDER BY first_seen_at LIMIT 1000)",
            "DELETE FROM reader_return_daily
             WHERE cohort_day<(clock_timestamp() AT TIME ZONE 'UTC')::date-180",
            // Preserve the first anonymous epoch as the true observation-start
            // marker; the remaining old coverage has no personal information.
            "DELETE FROM reader_return_coverage e
             WHERE e.ended_at<clock_timestamp()-interval '180 days'
               AND e.id<>(SELECT min(id) FROM reader_return_coverage)
               AND NOT EXISTS(SELECT 1 FROM reader_return_subjects s WHERE s.coverage_epoch_id=e.id)",
        ] {
            sqlx::query(statement)
                .execute(&mut **tx)
                .await
                .map_err(unavailable)?;
        }
    let ready: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM reader_activity_days
               WHERE first_seen_at<clock_timestamp()-interval '30 days')
             AND NOT EXISTS(SELECT 1 FROM reader_return_subjects
               WHERE cohort_day+30<(now() AT TIME ZONE 'UTC')::date)",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(unavailable)?;
    Ok(ready)
}

// Lock the F consent row before C cohort state, matching the progress writer.
// This transfer and temporal-state erasure are one transaction: a crash or retry
// cannot increment anonymous results twice or leave a half-anonymized subject.
const FINALIZE: &str = r#"
WITH chosen AS MATERIALIZED (
  SELECT s.subject_id,s.cohort_day,s.returned_d1,s.returned_d7,s.returned_d30
  FROM reader_activity_consent consent
  JOIN reader_return_subjects s ON s.subject_id=consent.subject_id
  WHERE s.cohort_day+30<(now() AT TIME ZONE 'UTC')::date
  ORDER BY s.cohort_day,s.subject_id LIMIT 500
  FOR UPDATE OF consent,s SKIP LOCKED
), horizons AS (
  SELECT c.*,h.day,h.did_return,
         EXISTS(SELECT 1 FROM reader_return_coverage e
           WHERE e.started_at<=((c.cohort_day+h.day)::timestamp AT TIME ZONE 'UTC')
             AND COALESCE(e.ended_at,e.last_heartbeat_at)
                 >=((c.cohort_day+h.day+1)::timestamp AT TIME ZONE 'UTC')) AS eligible
  FROM chosen c CROSS JOIN LATERAL
    (VALUES(1,c.returned_d1),(7,c.returned_d7),(30,c.returned_d30)) h(day,did_return)
), transferred AS (
  INSERT INTO reader_return_daily AS saved(cohort_day,day,enrolled,eligible,returned,unknown)
  SELECT cohort_day,day,count(*),count(*) FILTER(WHERE eligible),
         count(*) FILTER(WHERE eligible AND did_return),count(*) FILTER(WHERE NOT eligible)
  FROM horizons GROUP BY cohort_day,day
  ON CONFLICT(cohort_day,day) DO UPDATE SET
    enrolled=saved.enrolled+EXCLUDED.enrolled,
    eligible=saved.eligible+EXCLUDED.eligible,
    returned=saved.returned+EXCLUDED.returned,
    unknown=saved.unknown+EXCLUDED.unknown
  RETURNING cohort_day
), erased_activity AS (
  DELETE FROM reader_activity_days d USING chosen c WHERE d.subject_id=c.subject_id
  RETURNING d.subject_id
)
UPDATE reader_return_subjects s
SET cohort_day=NULL,coverage_epoch_id=NULL,returned_d1=NULL,returned_d7=NULL,returned_d30=NULL
WHERE s.subject_id IN (SELECT subject_id FROM chosen)
  AND EXISTS(SELECT 1 FROM transferred t WHERE t.cohort_day=s.cohort_day)
"#;
