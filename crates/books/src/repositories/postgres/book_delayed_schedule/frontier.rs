//! S058: NEwBOO first-reader-invisible frontier for a claimed book slot.
//! Binding never releases a chapter or creates reader/publication events.
use super::super::PgAuthoring;
use super::BookSlotClaim;
use crate::services::authoring::{valid_command_key, AuthorError};
use sqlx::{Postgres, Row, Transaction};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookSlotFrontier {
    Ready(i64),
    Blocked {
        chapter_id: Option<i64>,
        reason: &'static str,
    },
    Empty,
}

pub(super) async fn finish_terminal(
    tx: &mut Transaction<'_, Postgres>,
    claim: &BookSlotClaim,
    token: &str,
    chapter_id: Option<i64>,
    status: &str,
    reason: Option<&str>,
) -> Result<(), AuthorError> {
    let changed = sqlx::query(
        "UPDATE book_delayed_publication_occurrences
         SET chapter_id=$5,status=$6,last_error_code=$7,
             completed_at=clock_timestamp(),
             processing_token=NULL,processing_lease_expires_at=NULL
         WHERE book_id=$1 AND schedule_revision=$2
           AND local_date=$3::date AND slot_index=$4
           AND target_at=$8::timestamptz
           AND status='processing' AND processing_token=$9
           AND attempt_count=$10
           AND processing_lease_expires_at>clock_timestamp()",
    )
    .bind(claim.book_id)
    .bind(claim.schedule_revision)
    .bind(&claim.local_date)
    .bind(claim.slot_index)
    .bind(chapter_id)
    .bind(status)
    .bind(reason)
    .bind(&claim.target_utc)
    .bind(token)
    .bind(claim.attempt_count)
    .execute(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?
    .rows_affected();
    if changed != 1 {
        return Err(AuthorError::Conflict);
    }
    Ok(())
}

/// Move an empty occurrence to the next ordered Moscow slot in the same
/// transaction. This preserves duplicate clock times via their slot indices.
pub(super) async fn advance_terminal_slot(
    tx: &mut Transaction<'_, Postgres>,
    claim: &BookSlotClaim,
) -> Result<(), AuthorError> {
    let next = sqlx::query(
        "SELECT slot_index,minute_of_day FROM book_delayed_publication_slots
         WHERE book_id=$1 AND slot_index>$2 ORDER BY slot_index LIMIT 1",
    )
    .bind(claim.book_id)
    .bind(claim.slot_index)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    let (next_index, minute, days): (i32, i32, i32) = if let Some(next) = next {
        (
            next.try_get("slot_index")
                .map_err(|_| AuthorError::Unavailable)?,
            next.try_get("minute_of_day")
                .map_err(|_| AuthorError::Unavailable)?,
            0,
        )
    } else {
        let first = sqlx::query(
            "SELECT slot_index,minute_of_day FROM book_delayed_publication_slots
             WHERE book_id=$1 ORDER BY slot_index LIMIT 1",
        )
        .bind(claim.book_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let interval: i32 = sqlx::query_scalar(
            "SELECT interval_days FROM book_delayed_publication_settings
             WHERE book_id=$1",
        )
        .bind(claim.book_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        (
            first
                .try_get("slot_index")
                .map_err(|_| AuthorError::Unavailable)?,
            first
                .try_get("minute_of_day")
                .map_err(|_| AuthorError::Unavailable)?,
            interval,
        )
    };
    let changed = sqlx::query(
        "UPDATE book_delayed_publication_settings
         SET next_slot_at=(
             (($1::date+$2::integer)::timestamp
                + make_interval(mins=>$3::integer)) AT TIME ZONE 'Europe/Moscow'
         ), next_slot_index=$4
         WHERE book_id=$5 AND enabled AND schedule_revision=$6
           AND next_slot_index=$7 AND next_slot_at=$8::timestamptz",
    )
    .bind(&claim.local_date)
    .bind(days)
    .bind(minute)
    .bind(next_index)
    .bind(claim.book_id)
    .bind(claim.schedule_revision)
    .bind(claim.slot_index)
    .bind(&claim.target_utc)
    .execute(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?
    .rows_affected();
    if changed != 1 {
        return Err(AuthorError::Conflict);
    }
    Ok(())
}

impl PgAuthoring {
    /// Under one Book -> schedule -> occurrence transaction lock, bind the
    /// first chapter still invisible to readers. Never skip a held, unprepared,
    /// paid or separately scheduled chapter in favor of a later chapter.
    /// An already bound chapter is stable across lease recovery.
    pub async fn bind_book_slot_frontier(
        &self,
        claim: &BookSlotClaim,
        token: &str,
    ) -> Result<BookSlotFrontier, AuthorError> {
        if !valid_command_key(token)
            || claim.book_id <= 0
            || claim.schedule_revision <= 0
            || claim.attempt_count <= 0
            || !(0..100).contains(&claim.slot_index)
        {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(claim.book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let visibility = visibility.ok_or(AuthorError::Missing)?;
        let schedule: Option<i64> = sqlx::query_scalar(
            "SELECT book_id FROM book_delayed_publication_settings
             WHERE book_id=$1 AND enabled AND schedule_revision=$2
               AND next_slot_index=$3 AND next_slot_at=$4::timestamptz
             FOR UPDATE",
        )
        .bind(claim.book_id)
        .bind(claim.schedule_revision)
        .bind(claim.slot_index)
        .bind(&claim.target_utc)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if schedule.is_none() {
            return Err(AuthorError::Conflict);
        }
        let occurrence = sqlx::query(
            "SELECT chapter_id FROM book_delayed_publication_occurrences
             WHERE book_id=$1 AND schedule_revision=$2
               AND local_date=$3::date AND slot_index=$4
               AND target_at=$5::timestamptz
               AND status='processing' AND processing_token=$6
               AND attempt_count=$7
               AND processing_lease_expires_at>clock_timestamp()
             FOR UPDATE",
        )
        .bind(claim.book_id)
        .bind(claim.schedule_revision)
        .bind(&claim.local_date)
        .bind(claim.slot_index)
        .bind(&claim.target_utc)
        .bind(token)
        .bind(claim.attempt_count)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Conflict)?;
        let bound: Option<i64> = occurrence
            .try_get("chapter_id")
            .map_err(|_| AuthorError::Unavailable)?;
        if let Some(id) = bound {
            tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
            return Ok(BookSlotFrontier::Ready(id));
        }
        if visibility != "public" {
            finish_terminal(
                &mut tx,
                claim,
                token,
                None,
                "blocked",
                Some("book_not_public"),
            )
            .await?;
            tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
            return Ok(BookSlotFrontier::Blocked {
                chapter_id: None,
                reason: "book_not_public",
            });
        }
        let first = sqlx::query(
            "SELECT c.id,c.visibility,c.access_level,c.price_amount,c.admin_hold,
                    (c.reader_released_at IS NULL
                     OR c.reader_released_at<=clock_timestamp()) AS time_allowed,
                    d.version_id AS working_version,
                    (SELECT v.id FROM chapter_versions v
                     WHERE v.chapter_id=c.id AND v.version_no=c.edit_revision LIMIT 1)
                        AS paid_version,
                    (SELECT EXISTS (
                        SELECT 1 FROM chapter_versions v
                        JOIN storage_objects o ON o.id=v.storage_object_id
                        JOIN chapter_reader_manifests m
                          ON m.chapter_id=c.id AND m.version_id=v.id
                         AND m.storage_object_id=o.id
                        WHERE v.chapter_id=c.id AND v.version_no=c.edit_revision
                          AND o.state='attached' AND o.object_kind='chapter'
                          AND o.verified_at IS NOT NULL
                    )) AS paid_ready,
                    (p.version_id IS NOT NULL) AS has_publication,
                    (scheduled.chapter_id IS NOT NULL) AS manually_scheduled
             FROM chapters c
             LEFT JOIN chapter_publications p ON p.chapter_id=c.id
             LEFT JOIN chapter_working_drafts d ON d.chapter_id=c.id
             LEFT JOIN chapter_scheduled_releases scheduled
                 ON scheduled.chapter_id=c.id
                AND scheduled.status IN ('pending','claimed')
             WHERE c.book_id=$1 AND NOT c.is_deleted
               AND NOT (c.visibility='public' AND p.version_id IS NOT NULL
                        AND NOT c.admin_hold
                        AND (c.reader_released_at IS NULL
                             OR c.reader_released_at<=clock_timestamp()))
             ORDER BY c.chapter_no,c.id LIMIT 1 FOR UPDATE OF c",
        )
        .bind(claim.book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(row) = first else {
            finish_terminal(&mut tx, claim, token, None, "empty", None).await?;
            advance_terminal_slot(&mut tx, claim).await?;
            tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
            return Ok(BookSlotFrontier::Empty);
        };
        let id: i64 = row.try_get("id").map_err(|_| AuthorError::Unavailable)?;
        let held: bool = row
            .try_get("admin_hold")
            .map_err(|_| AuthorError::Unavailable)?;
        let time_allowed: bool = row
            .try_get("time_allowed")
            .map_err(|_| AuthorError::Unavailable)?;
        let chapter_visibility: String = row
            .try_get("visibility")
            .map_err(|_| AuthorError::Unavailable)?;
        let access: String = row
            .try_get("access_level")
            .map_err(|_| AuthorError::Unavailable)?;
        let price: i64 = row
            .try_get("price_amount")
            .map_err(|_| AuthorError::Unavailable)?;
        let working: Option<i64> = row
            .try_get("working_version")
            .map_err(|_| AuthorError::Unavailable)?;
        let paid_version: Option<i64> = row
            .try_get("paid_version")
            .map_err(|_| AuthorError::Unavailable)?;
        let paid_ready: bool = row
            .try_get("paid_ready")
            .map_err(|_| AuthorError::Unavailable)?;
        let published: bool = row
            .try_get("has_publication")
            .map_err(|_| AuthorError::Unavailable)?;
        let manual: bool = row
            .try_get("manually_scheduled")
            .map_err(|_| AuthorError::Unavailable)?;
        let blocked_reason = if held {
            Some("chapter_admin_hold")
        } else if !time_allowed {
            Some("future_reader_release")
        } else if chapter_visibility != "draft" || published {
            Some("chapter_not_pending")
        } else if manual {
            Some("manual_schedule_active")
        } else if access == "paid" && price > 0 {
            if !paid_ready || paid_version.is_none() {
                Some("paid_version_not_prepared")
            } else {
                None
            }
        } else if access != "free" || price != 0 {
            Some("invalid_chapter_access")
        } else if working.is_none() {
            Some("missing_working_version")
        } else {
            None
        };
        if let Some(reason) = blocked_reason {
            finish_terminal(&mut tx, claim, token, Some(id), "blocked", Some(reason)).await?;
            tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
            return Ok(BookSlotFrontier::Blocked {
                chapter_id: Some(id),
                reason,
            });
        }
        let changed = sqlx::query(
            "UPDATE book_delayed_publication_occurrences
             SET chapter_id=$5
             WHERE book_id=$1 AND schedule_revision=$2
               AND local_date=$3::date AND slot_index=$4
               AND status='processing' AND processing_token=$6
               AND attempt_count=$7
               AND processing_lease_expires_at>clock_timestamp()
               AND chapter_id IS NULL",
        )
        .bind(claim.book_id)
        .bind(claim.schedule_revision)
        .bind(&claim.local_date)
        .bind(claim.slot_index)
        .bind(id)
        .bind(token)
        .bind(claim.attempt_count)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .rows_affected();
        if changed != 1 {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(BookSlotFrontier::Ready(id))
    }
}
