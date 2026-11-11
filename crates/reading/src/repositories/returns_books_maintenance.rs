//! Independent book coverage and exact-tuple anonymization in one transaction.
use super::{unavailable, PgReadingReturns, ReadingReturnsError};
use sqlx::{Postgres, Transaction};
use std::time::{Duration, Instant};

impl PgReadingReturns {
    pub async fn start_book_collection(&self, enabled: bool) -> Result<(), ReadingReturnsError> {
        self.supervised_book_tick(enabled, true).await
    }

    pub async fn tick_book_collection(&self, enabled: bool) -> Result<(), ReadingReturnsError> {
        self.supervised_book_tick(enabled, false).await
    }

    async fn supervised_book_tick(
        &self,
        enabled: bool,
        restart: bool,
    ) -> Result<(), ReadingReturnsError> {
        match self.book_cycle(enabled, restart).await {
            Ok(()) => Ok(()),
            Err(error) => {
                sqlx::query("UPDATE reader_book_return_coverage SET ended_at=last_heartbeat_at WHERE ended_at IS NULL")
                    .execute(&self.pool).await.map_err(unavailable)?;
                Err(error)
            }
        }
    }

    async fn book_cycle(&self, enabled: bool, restart: bool) -> Result<(), ReadingReturnsError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('reader-book-return-coverage-v1',0))",
        )
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        sqlx::query(
            "UPDATE reader_book_return_coverage SET ended_at=last_heartbeat_at
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
                 INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at)
                 SELECT at,at FROM observed
                 ON CONFLICT ((true)) WHERE ended_at IS NULL
                 DO UPDATE SET last_heartbeat_at=EXCLUDED.last_heartbeat_at",
            )
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        }
        // The first tick after midnight publishes the heartbeat and finalized
        // D30 together. It cannot freeze a still-fresh day as falsely unknown.
        if !maintain(&mut tx).await? {
            sqlx::query("UPDATE reader_book_return_coverage SET ended_at=last_heartbeat_at WHERE ended_at IS NULL")
                .execute(&mut *tx).await.map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)
    }
}

async fn maintain(tx: &mut Transaction<'_, Postgres>) -> Result<bool, ReadingReturnsError> {
    let started = Instant::now();
    for _ in 0..16 {
        sqlx::query(FINALIZE)
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        for statement in [
            "DELETE FROM reader_book_return_daily
             WHERE cohort_day<=(clock_timestamp() AT TIME ZONE 'UTC')::date-180",
            // Anonymous start date remains as the true beginning of collection.
            "DELETE FROM reader_book_return_coverage
             WHERE ended_at<clock_timestamp()-interval '180 days'
               AND id<>(SELECT min(id) FROM reader_book_return_coverage)",
        ] {
            sqlx::query(statement)
                .execute(&mut **tx)
                .await
                .map_err(unavailable)?;
        }
        let ready: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS(SELECT 1 FROM reader_book_return_subjects
               WHERE cohort_day+30<(now() AT TIME ZONE 'UTC')::date)",
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(unavailable)?;
        if ready {
            return Ok(true);
        }
        if started.elapsed() >= Duration::from_millis(500) {
            break;
        }
    }
    Ok(false)
}

// Consent first, then exact episode, matching the accepted progress writer.
// A successor for the same subject/book may already exist and must survive.
const FINALIZE: &str = r#"
WITH chosen AS MATERIALIZED (
  SELECT s.subject_id,s.book_id,s.cohort_day,s.returned_d7,s.returned_d30
  FROM reader_activity_consent consent
  JOIN reader_book_return_subjects s ON s.subject_id=consent.subject_id
  WHERE s.cohort_day+30<(now() AT TIME ZONE 'UTC')::date
  ORDER BY s.cohort_day,s.subject_id,s.book_id LIMIT 500
  FOR UPDATE OF consent,s SKIP LOCKED
), horizons AS (
  SELECT c.*,h.day,h.did_return,
         EXISTS(SELECT 1 FROM reader_book_return_coverage e
           WHERE e.started_at<=((c.cohort_day+h.day)::timestamp AT TIME ZONE 'UTC')
             AND COALESCE(e.ended_at,e.last_heartbeat_at)
                 >=((c.cohort_day+h.day+1)::timestamp AT TIME ZONE 'UTC')) AS eligible
  FROM chosen c CROSS JOIN LATERAL
    (VALUES(7,c.returned_d7),(30,c.returned_d30)) h(day,did_return)
), transferred AS (
  INSERT INTO reader_book_return_daily AS saved(book_id,cohort_day,day,enrolled,eligible,returned,unknown)
  SELECT book_id,cohort_day,day,count(*),count(*) FILTER(WHERE eligible),
         count(*) FILTER(WHERE eligible AND did_return),count(*) FILTER(WHERE NOT eligible)
  FROM horizons GROUP BY book_id,cohort_day,day
  ON CONFLICT(book_id,cohort_day,day) DO UPDATE SET
    enrolled=saved.enrolled+EXCLUDED.enrolled,
    eligible=saved.eligible+EXCLUDED.eligible,
    returned=saved.returned+EXCLUDED.returned,
    unknown=saved.unknown+EXCLUDED.unknown
  RETURNING book_id,cohort_day
)
DELETE FROM reader_book_return_subjects s USING chosen c
WHERE s.subject_id=c.subject_id AND s.book_id=c.book_id AND s.cohort_day=c.cohort_day
  AND EXISTS(SELECT 1 FROM transferred t WHERE t.book_id=s.book_id AND t.cohort_day=s.cohort_day)
"#;
