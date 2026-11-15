//! Optional day collection runs after C accepted a positive, validated advance.
//! A failed derivative write rolls back only its savepoint and breaks coverage.
use crate::services::{progress::ReadingPlace, sync::SyncError};
use sqlx::{Postgres, Transaction};

pub(in crate::repositories) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
    place: ReadingPlace,
    collect: bool,
) -> Result<(), SyncError> {
    if !collect || (place.block_index == 1 && place.char_offset == 0) {
        return Ok(());
    }
    sqlx::query("SAVEPOINT reader_return_capture")
        .execute(&mut **tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
    if capture(tx, actor, book).await.is_err() {
        sqlx::query("ROLLBACK TO SAVEPOINT reader_return_capture")
            .execute(&mut **tx)
            .await
            .map_err(|_| SyncError::Unavailable)?;
        // A successful saved position without its optional day must never be
        // silently interpreted as a fully observed zero-return interval.
        sqlx::query(
            "UPDATE reader_return_coverage SET ended_at=last_heartbeat_at
             WHERE ended_at IS NULL",
        )
        .execute(&mut **tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
    }
    sqlx::query("RELEASE SAVEPOINT reader_return_capture")
        .execute(&mut **tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
    Ok(())
}

async fn capture(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
) -> Result<(), sqlx::Error> {
    // F's approved projection remains row-lockable. Opt-out, opt-in and account
    // erasure serialize with both this insert and eventual anonymization.
    let subject = sqlx::query_scalar::<_, String>(
        "SELECT subject_id::text FROM reader_activity_consent WHERE user_id=$1 FOR UPDATE",
    )
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(subject) = subject else {
        return Ok(());
    };
    let finished: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM reader_return_subjects WHERE subject_id=$1::uuid
           AND (cohort_day IS NULL OR cohort_day+30<(clock_timestamp() AT TIME ZONE 'UTC')::date))",
    )
    .bind(&subject)
    .fetch_one(&mut **tx)
    .await?;
    if finished {
        // The one observed cohort is complete. Keep only its dateless marker;
        // collecting further book/day history would serve no return metric.
        return Ok(());
    }
    let coverage = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM reader_return_coverage
         WHERE ended_at IS NULL
           AND last_heartbeat_at>=clock_timestamp()-interval '60 seconds'
         FOR SHARE",
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some(coverage) = coverage else {
        // The supervised collector must first prove it is running. A progress
        // request is not allowed to manufacture missing coverage retrospectively.
        return Ok(());
    };
    sqlx::query(
        "WITH observed AS MATERIALIZED (SELECT clock_timestamp() AS at),
         activity AS (
           INSERT INTO reader_activity_days(subject_id,book_id,activity_day,first_seen_at)
           SELECT $1::uuid,$2,(at AT TIME ZONE 'UTC')::date,at FROM observed
           ON CONFLICT(subject_id,book_id,activity_day) DO NOTHING
         )
         INSERT INTO reader_return_subjects AS saved
           (subject_id,cohort_day,coverage_epoch_id,returned_d1,returned_d7,returned_d30)
         SELECT $1::uuid,(at AT TIME ZONE 'UTC')::date,$3,false,false,false FROM observed
         ON CONFLICT(subject_id) DO UPDATE SET
           returned_d1=CASE
             WHEN saved.cohort_day+1=EXCLUDED.cohort_day THEN true ELSE saved.returned_d1 END,
           returned_d7=CASE
             WHEN saved.cohort_day+7=EXCLUDED.cohort_day THEN true ELSE saved.returned_d7 END,
           returned_d30=CASE
             WHEN saved.cohort_day+30=EXCLUDED.cohort_day THEN true ELSE saved.returned_d30 END",
    )
    .bind(subject)
    .bind(book)
    .bind(coverage)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
