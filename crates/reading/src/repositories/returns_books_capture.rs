//! An independent optional book derivative after C accepted positive progress.
use crate::services::{progress::ReadingPlace, sync::SyncError};
use sqlx::{Postgres, Transaction};

pub(super) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
    place: ReadingPlace,
    collect: bool,
) -> Result<(), SyncError> {
    if !collect || (place.block_index == 1 && place.char_offset == 0) {
        return Ok(());
    }
    sqlx::query("SAVEPOINT reader_book_return_capture")
        .execute(&mut **tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
    if capture(tx, actor, book).await.is_err() {
        sqlx::query("ROLLBACK TO SAVEPOINT reader_book_return_capture")
            .execute(&mut **tx)
            .await
            .map_err(|_| SyncError::Unavailable)?;
        sqlx::query(
            "UPDATE reader_book_return_coverage SET ended_at=last_heartbeat_at
             WHERE ended_at IS NULL",
        )
        .execute(&mut **tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
    }
    sqlx::query("RELEASE SAVEPOINT reader_book_return_capture")
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
    // Match consent rotation and finalization lock order. This also serializes
    // two devices choosing whether a new bounded book episode is necessary.
    let subject = sqlx::query_scalar::<_, String>(
        "SELECT subject_id::text FROM reader_activity_consent WHERE user_id=$1 FOR UPDATE",
    )
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(subject) = subject else {
        return Ok(());
    };
    let coverage = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM reader_book_return_coverage WHERE ended_at IS NULL
           AND last_heartbeat_at>=clock_timestamp()-interval '60 seconds' FOR SHARE",
    )
    .fetch_optional(&mut **tx)
    .await?;
    if coverage.is_none() {
        return Ok(());
    }
    sqlx::query(
        "WITH observed AS MATERIALIZED (
           SELECT (clock_timestamp() AT TIME ZONE 'UTC')::date AS today
         ), current_episode AS (
           SELECT cohort_day FROM reader_book_return_subjects CROSS JOIN observed
           WHERE subject_id=$1::uuid AND book_id=$2
             AND cohort_day>=today-30 AND cohort_day<=today
           ORDER BY cohort_day DESC LIMIT 1
         )
         INSERT INTO reader_book_return_subjects AS saved
           (subject_id,book_id,cohort_day,returned_d7,returned_d30)
         SELECT $1::uuid,$2,COALESCE((SELECT cohort_day FROM current_episode),today),
                COALESCE((SELECT cohort_day+7=today FROM current_episode),false),
                COALESCE((SELECT cohort_day+30=today FROM current_episode),false)
         FROM observed
         ON CONFLICT(subject_id,book_id,cohort_day) DO UPDATE SET
           returned_d7=saved.returned_d7 OR EXCLUDED.returned_d7,
           returned_d30=saved.returned_d30 OR EXCLUDED.returned_d30",
    )
    .bind(subject)
    .bind(book)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
