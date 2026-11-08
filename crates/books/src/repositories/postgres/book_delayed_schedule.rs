//! B/S058: book-owned, Moscow-time release configuration from NEwBOO.
//! A saved schedule is not itself a release or a promise of an active worker.
mod frontier;
mod occurrences;
mod release;
use super::PgAuthoring;
use crate::services::{
    authoring::AuthorError,
    book_delayed_schedule::{
        on_cadence_day, parse_publication_times, publication_time_text, BookDelayedSchedule,
    },
};
pub use frontier::BookSlotFrontier;
pub use occurrences::BookSlotClaim;
pub use release::BookSlotReleaseOutcome;
use sqlx::{Postgres, Row, Transaction};

async fn require_manager(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
    write: bool,
) -> Result<(), AuthorError> {
    if actor <= 0 || book <= 0 {
        return Err(AuthorError::Missing);
    }
    // Lock order: Book first, then active membership. Prevents revoked users
    // from racing an in-flight settings mutation or owner transfer.
    let sql = if write {
        "SELECT id FROM books WHERE id=$1 AND NOT admin_hold AND NOT is_deleted FOR UPDATE"
    } else {
        "SELECT id FROM books WHERE id=$1 AND NOT admin_hold AND NOT is_deleted FOR SHARE"
    };
    let locked: Option<i64> = sqlx::query_scalar(sql)
        .bind(book)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
    locked.ok_or(AuthorError::Missing)?;
    let role: Option<String> = sqlx::query_scalar(
        "SELECT m.role FROM book_members m JOIN books b ON b.id=m.book_id
         WHERE m.book_id=$1 AND m.user_id=$2 AND m.is_active
           AND (m.role IN ('translator','moderator')
                OR (m.role='owner' AND b.owner_user_id=$2))
         FOR SHARE OF m",
    )
    .bind(book)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    role.ok_or(AuthorError::Missing)?;
    Ok(())
}

async fn load_schedule(
    tx: &mut Transaction<'_, Postgres>,
    book: i64,
) -> Result<(BookDelayedSchedule, Option<i32>), AuthorError> {
    let row = sqlx::query(
        r#"SELECT enabled,interval_days,schedule_revision,
                  (anchor_date - DATE '1970-01-01')::integer AS anchor_day,
                  to_char(anchor_date,'YYYY-MM-DD') AS anchor_date,
                  to_char(next_slot_at AT TIME ZONE 'UTC',
                          'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS next_slot_utc,
                  next_slot_index
           FROM book_delayed_publication_settings WHERE book_id=$1"#,
    )
    .bind(book)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    let Some(row) = row else {
        return Ok((BookDelayedSchedule::default(), None));
    };
    let minutes: Vec<i32> = sqlx::query_scalar(
        "SELECT minute_of_day FROM book_delayed_publication_slots
         WHERE book_id=$1 ORDER BY slot_index LIMIT 101",
    )
    .bind(book)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    let times = minutes
        .into_iter()
        .map(publication_time_text)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        BookDelayedSchedule {
            enabled: row
                .try_get("enabled")
                .map_err(|_| AuthorError::Unavailable)?,
            interval_days: row
                .try_get("interval_days")
                .map_err(|_| AuthorError::Unavailable)?,
            publication_times: times,
            schedule_revision: row
                .try_get("schedule_revision")
                .map_err(|_| AuthorError::Unavailable)?,
            anchor_date: row
                .try_get("anchor_date")
                .map_err(|_| AuthorError::Unavailable)?,
            next_slot_utc: row
                .try_get("next_slot_utc")
                .map_err(|_| AuthorError::Unavailable)?,
            next_slot_index: row
                .try_get("next_slot_index")
                .map_err(|_| AuthorError::Unavailable)?,
        },
        row.try_get("anchor_day")
            .map_err(|_| AuthorError::Unavailable)?,
    ))
}

/// The first UTC slot at or after the current instant; duplicate Moscow times
/// retain different slot indices. PostgreSQL owns the IANA time zone database.
async fn next_slot(
    tx: &mut Transaction<'_, Postgres>,
    start_day: i32,
    interval: i32,
    minutes: &[i32],
) -> Result<(String, i32), AuthorError> {
    let row = sqlx::query(
        r#"WITH slots AS (
             SELECT slot_minute, (ordinal - 1)::integer AS slot_index
             FROM unnest($1::integer[]) WITH ORDINALITY AS slot(slot_minute, ordinal)
           ), candidates AS (
             SELECT slot_index,
                    ((DATE '1970-01-01' + ($2::integer + $3::integer * day_offset))::timestamp
                      + make_interval(mins => slot_minute)) AT TIME ZONE 'Europe/Moscow' AS target
             FROM slots CROSS JOIN generate_series(0,1) AS day_offset
           )
           SELECT slot_index,
                  to_char(target AT TIME ZONE 'UTC',
                          'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS next_slot_utc
           FROM candidates WHERE target >= clock_timestamp()
           ORDER BY target,slot_index LIMIT 1"#,
    )
    .bind(minutes)
    .bind(start_day)
    .bind(interval)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?
    .ok_or(AuthorError::Unavailable)?;
    Ok((
        row.try_get("next_slot_utc")
            .map_err(|_| AuthorError::Unavailable)?,
        row.try_get("slot_index")
            .map_err(|_| AuthorError::Unavailable)?,
    ))
}

impl PgAuthoring {
    /// Startup proof for the book-slot worker. An operator-enabled worker
    /// must refuse to start against a schema missing any S058 occurrence,
    /// cursor, or schedule table, without changing DB state.
    pub async fn probe_book_schedule_worker(&self) -> Result<(), AuthorError> {
        sqlx::query(
            "SELECT s.book_id,s.schedule_revision,s.next_slot_at,s.next_slot_index,
                    o.processing_token,o.attempt_count,o.processing_lease_expires_at,
                    sl.minute_of_day
             FROM book_delayed_publication_settings s
             LEFT JOIN book_delayed_publication_occurrences o ON o.book_id=s.book_id
             LEFT JOIN book_delayed_publication_slots sl ON sl.book_id=s.book_id
             LIMIT 0",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }

    pub async fn get_book_delayed_schedule(
        &self,
        actor: i64,
        book: i64,
    ) -> Result<BookDelayedSchedule, AuthorError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        require_manager(&mut tx, actor, book, false).await?;
        let (current, _) = load_schedule(&mut tx, book).await?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(current)
    }

    /// Active Book members can change schedule settings, even on draft books.
    /// All edits serialize on the Book row; an identical write is idempotent.
    /// Does not create occurrences, release chapters, or grant access.
    pub async fn replace_book_delayed_schedule(
        &self,
        actor: i64,
        book: i64,
        enabled: bool,
        interval_days: i32,
        publication_times: &[String],
    ) -> Result<BookDelayedSchedule, AuthorError> {
        if interval_days < 1 {
            return Err(AuthorError::Invalid);
        }
        let minutes = parse_publication_times(publication_times)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        require_manager(&mut tx, actor, book, true).await?;
        let (current, prior_anchor) = load_schedule(&mut tx, book).await?;
        if current.schedule_revision > 0
            && current.enabled == enabled
            && current.interval_days == interval_days
            && current.publication_times == publication_times
        {
            tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
            return Ok(current);
        }

        let mut anchor_day = None;
        let mut next_time = None;
        let mut next_index = None;
        if enabled {
            let today: i32 = sqlx::query_scalar(
                "SELECT ((clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date
                         - DATE '1970-01-01')::integer",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            let preserve = current.enabled && current.interval_days == interval_days;
            let anchor = if preserve {
                prior_anchor.ok_or(AuthorError::Unavailable)?
            } else {
                // NEwBOO anchors new cadence today if any slot remains;
                // otherwise it starts tomorrow (not interval_days later).
                let remaining: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                       SELECT 1 FROM unnest($1::integer[]) AS m(minute_of_day)
                       WHERE (((clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date::timestamp
                           + make_interval(mins => m.minute_of_day))
                           AT TIME ZONE 'Europe/Moscow') >= clock_timestamp()
                     )",
                )
                .bind(&minutes)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| AuthorError::Unavailable)?;
                if remaining {
                    today
                } else {
                    today + 1
                }
            };
            let first_day = on_cadence_day(anchor, today, interval_days)?;
            let (target, index) = next_slot(&mut tx, first_day, interval_days, &minutes).await?;
            anchor_day = Some(anchor);
            next_time = Some(target);
            next_index = Some(index);
        }
        let revision = current.schedule_revision + 1;
        sqlx::query(
            "INSERT INTO book_delayed_publication_settings
                 (book_id,enabled,interval_days,anchor_date,schedule_revision,
                  next_slot_at,next_slot_index,updated_at,updated_by)
             VALUES ($1,$2,$3,DATE '1970-01-01' + $4::integer,$5,
                     $6::text::timestamptz,$7,clock_timestamp(),$8)
             ON CONFLICT(book_id) DO UPDATE SET
                 enabled=EXCLUDED.enabled,interval_days=EXCLUDED.interval_days,
                 anchor_date=EXCLUDED.anchor_date,schedule_revision=EXCLUDED.schedule_revision,
                 next_slot_at=EXCLUDED.next_slot_at,next_slot_index=EXCLUDED.next_slot_index,
                 updated_at=EXCLUDED.updated_at,updated_by=EXCLUDED.updated_by",
        )
        .bind(book)
        .bind(enabled)
        .bind(interval_days)
        .bind(anchor_day)
        .bind(revision)
        .bind(&next_time)
        .bind(next_index)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query("DELETE FROM book_delayed_publication_slots WHERE book_id=$1")
            .bind(book)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        for (index, minute) in minutes.iter().enumerate() {
            sqlx::query(
                "INSERT INTO book_delayed_publication_slots(book_id,slot_index,minute_of_day)
                 VALUES($1,$2,$3)",
            )
            .bind(book)
            .bind(i32::try_from(index).map_err(|_| AuthorError::Invalid)?)
            .bind(minute)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        }
        let (saved, _) = load_schedule(&mut tx, book).await?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(saved)
    }
}
