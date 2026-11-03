//! S058: book-schedule release commits B-06, occurrence and cadence atomically.
//! The paid path activates only a separately G-verified, privately staged edition.
//! This module does not activate background workers.
use super::super::{chapter_release::release_pending_free_chapter_tx, PgAuthoring};
use crate::repositories::paid_storage::scheduled::release_staged_paid_chapter_tx;
use super::frontier::{advance_terminal_slot, finish_terminal};
use super::BookSlotClaim;
use crate::services::authoring::{valid_command_key, AuthorError};
use sqlx::Row;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookSlotReleaseOutcome {
    Released {
        chapter_id: i64,
        version_id: i64,
    },
    Blocked {
        chapter_id: i64,
        reason: &'static str,
    },
}

impl PgAuthoring {
    /// Complete one bound free or privately staged paid frontier.
    /// All locks and the unique first-release fact commit together with
    /// occurrence completion and the compare-and-swap schedule cursor.
    pub async fn release_bound_book_slot(
        &self,
        claim: &BookSlotClaim,
        token: &str,
    ) -> Result<BookSlotReleaseOutcome, AuthorError> {
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
        // Match author writes: Book -> settings -> occurrence -> Chapter.
        let book = sqlx::query(
            "SELECT visibility,owner_user_id FROM books
             WHERE id=$1 AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(claim.book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        let visible: String = book
            .try_get("visibility")
            .map_err(|_| AuthorError::Unavailable)?;
        let owner: Option<i64> = book
            .try_get("owner_user_id")
            .map_err(|_| AuthorError::Unavailable)?;
        let enabled: Option<i64> = sqlx::query_scalar(
            "SELECT book_id FROM book_delayed_publication_settings
             WHERE book_id=$1 AND enabled AND schedule_revision=$2
               AND next_slot_index=$3 AND next_slot_at=$4::timestamptz
               AND next_slot_at<=clock_timestamp()
             FOR UPDATE",
        )
        .bind(claim.book_id)
        .bind(claim.schedule_revision)
        .bind(claim.slot_index)
        .bind(&claim.target_utc)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if enabled.is_none() {
            return Err(AuthorError::Conflict);
        }
        let bound: Option<i64> = sqlx::query_scalar(
            "SELECT chapter_id FROM book_delayed_publication_occurrences
             WHERE book_id=$1 AND schedule_revision=$2
               AND local_date=$3::date AND slot_index=$4
               AND target_at=$5::timestamptz
               AND target_at<=clock_timestamp()
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
        let chapter_id = bound.ok_or(AuthorError::Conflict)?;
        let first = sqlx::query(
            "SELECT c.id,c.visibility,c.access_level,c.price_amount,
                    c.admin_hold,c.edit_revision,d.version_id AS working_version,
                    (SELECT v.id FROM chapter_versions v
                     WHERE v.chapter_id=c.id ORDER BY v.version_no DESC LIMIT 1)
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
                    (c.reader_released_at IS NULL
                      OR c.reader_released_at<=clock_timestamp()) AS time_allowed,
                    (p.version_id IS NOT NULL) AS published,
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
        let (reason, version, revision, access) = if let Some(first) = first {
            let first_id: i64 = first.try_get("id").map_err(|_| AuthorError::Unavailable)?;
            let visibility: String = first
                .try_get("visibility")
                .map_err(|_| AuthorError::Unavailable)?;
            let access: String = first
                .try_get("access_level")
                .map_err(|_| AuthorError::Unavailable)?;
            let price: i64 = first
                .try_get("price_amount")
                .map_err(|_| AuthorError::Unavailable)?;
            let hold: bool = first
                .try_get("admin_hold")
                .map_err(|_| AuthorError::Unavailable)?;
            let time_allowed: bool = first
                .try_get("time_allowed")
                .map_err(|_| AuthorError::Unavailable)?;
            let published: bool = first
                .try_get("published")
                .map_err(|_| AuthorError::Unavailable)?;
            let scheduled: bool = first
                .try_get("manually_scheduled")
                .map_err(|_| AuthorError::Unavailable)?;
            let version: Option<i64> = first
                .try_get("working_version")
                .map_err(|_| AuthorError::Unavailable)?;
            let revision: i64 = first
                .try_get("edit_revision")
                .map_err(|_| AuthorError::Unavailable)?;
            let paid_version: Option<i64> = first
                .try_get("paid_version")
                .map_err(|_| AuthorError::Unavailable)?;
            let paid_ready: bool = first
                .try_get("paid_ready")
                .map_err(|_| AuthorError::Unavailable)?;
            let reason = if first_id != chapter_id {
                Some("frontier_changed")
            } else if visible != "public" {
                Some("book_not_public")
            } else if owner.is_none() {
                Some("book_owner_missing")
            } else if hold {
                Some("chapter_admin_hold")
            } else if !time_allowed {
                Some("future_reader_release")
            } else if visibility != "draft" || published {
                Some("chapter_not_pending")
            } else if scheduled {
                Some("manual_schedule_active")
            } else if access == "paid" && price > 0 {
                if !paid_ready || paid_version.is_none() {
                    Some("paid_version_not_prepared")
                } else {
                    None
                }
            } else if access != "free" || price != 0 {
                Some("invalid_chapter_access")
            } else if version.is_none() {
                Some("missing_working_version")
            } else {
                None
            };
            (reason, if access == "paid" { paid_version } else { version }, revision, access)
        } else {
            (Some("frontier_changed"), None, 0, String::new())
        };
        if let Some(reason) = reason {
            finish_terminal(
                &mut tx,
                claim,
                token,
                Some(chapter_id),
                "blocked",
                Some(reason),
            )
            .await?;
            tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
            return Ok(BookSlotReleaseOutcome::Blocked { chapter_id, reason });
        }
        let owner = owner.ok_or(AuthorError::Conflict)?;
        let version_id = version.ok_or(AuthorError::Conflict)?;
        // The paid edition remains private until the same transaction
        // completes the publication and schedule cursor. D gates all reads.
        if access == "paid" {
            release_staged_paid_chapter_tx(
                &mut tx, owner, claim.book_id, chapter_id, revision, version_id,
            ).await?;
        } else {
            release_pending_free_chapter_tx(
                &mut tx, owner, claim.book_id, chapter_id, revision, version_id,
            ).await?;
        }
        finish_terminal(&mut tx, claim, token, Some(chapter_id), "completed", None).await?;
        advance_terminal_slot(&mut tx, claim).await?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(BookSlotReleaseOutcome::Released {
            chapter_id,
            version_id,
        })
    }
}
