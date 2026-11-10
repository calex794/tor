//! Read-only C aggregate for the independently authorized site administrator.
//! Canonical facts remain owned by C; no REC identity, profile or event is created.
mod intent;

use crate::services::audience::{
    validate_period, AudienceCounts, AudienceError, AudiencePeriod, AudienceReport,
};
use sqlx::{postgres::PgRow, PgPool, Row};

#[derive(Clone)]
pub struct PgAudience {
    pool: PgPool,
}

impl PgAudience {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Two adjacent equal intervals share one database snapshot and UTC anchor.
    /// Identity is used only inside COUNT(DISTINCT), never returned or persisted.
    pub async fn report(&self, days: i32) -> Result<AudienceReport, AudienceError> {
        validate_period(days)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        // A slow aggregate must not compete indefinitely with normal reading.
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let rows = sqlx::query(REPORT_SQL)
            .bind(days)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        if rows.len() != 2 {
            return Err(AudienceError::Unavailable);
        }
        let measured_at_utc = rows[0].try_get("measured_at").map_err(unavailable)?;
        let current = period(&rows[0])?;
        let previous = period(&rows[1])?;
        tx.commit().await.map_err(unavailable)?;
        Ok(AudienceReport {
            days,
            measured_at_utc,
            current,
            previous,
        })
    }
}

fn unavailable(_: sqlx::Error) -> AudienceError {
    AudienceError::Unavailable
}

fn period(row: &PgRow) -> Result<AudiencePeriod, AudienceError> {
    let counts = AudienceCounts {
        starts: row.try_get("starts").map_err(unavailable)?,
        completions: row.try_get("completions").map_err(unavailable)?,
        unique_readers: row.try_get("unique_readers").map_err(unavailable)?,
        unique_started: row.try_get("unique_started").map_err(unavailable)?,
        unique_completed: row.try_get("unique_completed").map_err(unavailable)?,
    };
    Ok(AudiencePeriod {
        from_utc: row.try_get("from_utc").map_err(unavailable)?,
        until_utc: row.try_get("until_utc").map_err(unavailable)?,
        available: true,
        counts: (counts.starts != 0 || counts.completions != 0).then_some(counts),
    })
}

// Only the immutable first C chapter fact plus its matching domain event is
// evidence. An arbitrary outbox payload, HTTP GET or delivery grant is not.
// Current book visibility and later purchase revocation do not rewrite a past
// authorized fact; no title, book ID, account ID, or paid content leaves SQL.
const REPORT_SQL: &str = r#"
WITH anchor AS (
    SELECT transaction_timestamp() AS at, $1::int * interval '24 hours' AS span
), periods AS (
    SELECT 0 AS ordinal, at-span AS from_at, at AS until_at, at FROM anchor
    UNION ALL
    SELECT 1, at-span*2, at-span, at FROM anchor
), evidence AS MATERIALIZED (
    SELECT c.user_id,c.started_at AS occurred_at,0 AS kind
    FROM reader_chapter_starts c
    JOIN outbox_events e ON e.id=c.source_event_id
    CROSS JOIN anchor a
    WHERE c.started_at>=a.at-a.span*2 AND c.started_at<a.at
      AND e.event_type='ChapterStartedV1' AND e.schema_version=1
      AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
      AND e.payload->>'user_id'=c.user_id::text
      AND e.payload->>'book_id'=c.book_id::text
      AND e.payload->>'chapter_id'=c.chapter_id::text
      AND e.payload->>'version_id'=c.first_version_id::text
      AND ((c.purchase_id IS NULL
            AND e.payload->>'authority'='C-verified-free-start'
            AND NOT(e.payload ? 'purchase_id'))
        OR (c.purchase_id IS NOT NULL
            AND e.payload->>'authority'='C-verified-purchased-start'
            AND e.payload->>'purchase_id'=c.purchase_id::text))
    UNION ALL
    SELECT c.user_id,c.completed_at,1
    FROM reader_chapter_completions c
    JOIN outbox_events e ON e.id=c.source_event_id
    CROSS JOIN anchor a
    WHERE c.completed_at>=a.at-a.span*2 AND c.completed_at<a.at
      AND e.event_type='ChapterCompletedV1' AND e.schema_version=1
      AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
      AND e.payload->>'user_id'=c.user_id::text
      AND e.payload->>'book_id'=c.book_id::text
      AND e.payload->>'chapter_id'=c.chapter_id::text
      AND e.payload->>'version_id'=c.first_version_id::text
      AND ((c.purchase_id IS NULL
            AND e.payload->>'authority'='C-verified-free-position'
            AND NOT(e.payload ? 'purchase_id'))
        OR (c.purchase_id IS NOT NULL
            AND e.payload->>'authority'='C-verified-purchased-position'
            AND e.payload->>'purchase_id'=c.purchase_id::text))
)
SELECT p.ordinal,
       to_char(p.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at,
       to_char(p.from_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS from_utc,
       to_char(p.until_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS until_utc,
       count(*) FILTER(WHERE e.kind=0) AS starts,
       count(*) FILTER(WHERE e.kind=1) AS completions,
       count(DISTINCT e.user_id) AS unique_readers,
       count(DISTINCT e.user_id) FILTER(WHERE e.kind=0) AS unique_started,
       count(DISTINCT e.user_id) FILTER(WHERE e.kind=1) AS unique_completed
FROM periods p LEFT JOIN evidence e
  ON e.occurred_at>=p.from_at AND e.occurred_at<p.until_at
GROUP BY p.ordinal,p.from_at,p.until_at,p.at
ORDER BY p.ordinal
"#;
