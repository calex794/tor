//! NEwBOO S058: persisted slot identity, lease and revision fence.
//! No chapter binding, publication or worker activation in this layer.
use super::super::PgAuthoring;
use crate::services::authoring::{valid_command_key, AuthorError};
use sqlx::Row;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookSlotClaim {
    pub book_id: i64,
    pub schedule_revision: i64,
    pub local_date: String,
    pub slot_index: i32,
    pub target_utc: String,
    pub attempt_count: i32,
}

impl PgAuthoring {
    /// Claim exactly one due schedule. Book is locked before the settings row,
    /// matching the author-settings lock order and avoiding transfer deadlocks.
    /// In-flight, blocked and terminal occurrences are never hot-looped.
    pub async fn claim_due_book_slot(
        &self,
        token: &str,
    ) -> Result<Option<BookSlotClaim>, AuthorError> {
        if !valid_command_key(token) {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let book: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b
             JOIN book_delayed_publication_settings s ON s.book_id=b.id
             WHERE s.enabled AND s.next_slot_at<=clock_timestamp()
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT EXISTS (
                   SELECT 1 FROM book_delayed_publication_occurrences o
                   WHERE o.book_id=s.book_id
                     AND o.schedule_revision=s.schedule_revision
                     AND o.target_at=s.next_slot_at
                     AND o.slot_index=s.next_slot_index
                     AND (o.status IN ('completed','empty','blocked','cancelled')
                       OR (o.status='processing'
                           AND o.processing_lease_expires_at>clock_timestamp()))
               )
             ORDER BY s.next_slot_at,b.id
             FOR UPDATE OF b SKIP LOCKED LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(book) = book else {
            return Ok(None);
        };
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT book_id FROM book_delayed_publication_settings
             WHERE book_id=$1 AND enabled AND next_slot_at<=clock_timestamp()
             FOR UPDATE",
        )
        .bind(book)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if active.is_none() {
            return Ok(None);
        }
        // Cursor identity is generated from the settings row under its lock.
        // Retrying the same cursor may never create a second occurrence.
        sqlx::query(
            "INSERT INTO book_delayed_publication_occurrences
                 (book_id,schedule_revision,local_date,slot_index,target_at)
             SELECT book_id,schedule_revision,
                    (next_slot_at AT TIME ZONE 'Europe/Moscow')::date,
                    next_slot_index,next_slot_at
             FROM book_delayed_publication_settings WHERE book_id=$1
             ON CONFLICT DO NOTHING",
        )
        .bind(book)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let row = sqlx::query(
            r#"UPDATE book_delayed_publication_occurrences o
               SET status='processing',
                   attempt_count=o.attempt_count+1,
                   processing_token=$2,
                   processing_lease_expires_at=clock_timestamp()+interval '5 minutes',
                   started_at=COALESCE(o.started_at,clock_timestamp()),
                   last_error_code=NULL
               FROM book_delayed_publication_settings s
               WHERE s.book_id=$1 AND o.book_id=s.book_id
                 AND o.schedule_revision=s.schedule_revision
                 AND o.target_at=s.next_slot_at
                 AND o.slot_index=s.next_slot_index
                 AND s.enabled AND s.next_slot_at<=clock_timestamp()
                 AND (o.status='pending'
                      OR (o.status='processing' AND
                          o.processing_lease_expires_at<=clock_timestamp()))
               RETURNING o.book_id,o.schedule_revision,o.slot_index,
                         to_char(o.local_date,'YYYY-MM-DD') AS local_date,
                         to_char(o.target_at AT TIME ZONE 'UTC',
                                'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS target_utc,
                         o.attempt_count"#,
        )
        .bind(book)
        .bind(token)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let claim = row
            .map(|row| {
                Ok(BookSlotClaim {
                    book_id: row
                        .try_get("book_id")
                        .map_err(|_| AuthorError::Unavailable)?,
                    schedule_revision: row
                        .try_get("schedule_revision")
                        .map_err(|_| AuthorError::Unavailable)?,
                    local_date: row
                        .try_get("local_date")
                        .map_err(|_| AuthorError::Unavailable)?,
                    slot_index: row
                        .try_get("slot_index")
                        .map_err(|_| AuthorError::Unavailable)?,
                    target_utc: row
                        .try_get("target_utc")
                        .map_err(|_| AuthorError::Unavailable)?,
                    attempt_count: row
                        .try_get("attempt_count")
                        .map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .transpose()?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(claim)
    }

    /// Verify that a processing lease still owns the exact book schedule
    /// revision and cursor. Future frontier/release code must call this under
    /// a Book transaction lock before mutating a chapter or publishing outbox.
    pub async fn book_slot_claim_is_current(
        &self,
        claim: &BookSlotClaim,
        token: &str,
    ) -> Result<bool, AuthorError> {
        if !valid_command_key(token) || claim.book_id <= 0 || claim.schedule_revision <= 0 {
            return Err(AuthorError::Invalid);
        }
        let current: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM book_delayed_publication_occurrences o
                JOIN book_delayed_publication_settings s ON s.book_id=o.book_id
                JOIN books b ON b.id=o.book_id
                WHERE o.book_id=$1 AND o.schedule_revision=$2
                  AND o.local_date=$3::date AND o.slot_index=$4
                  AND o.target_at=$5::timestamptz
                  AND o.status='processing' AND o.processing_token=$6
                  AND o.processing_lease_expires_at>clock_timestamp()
                  AND s.enabled AND s.schedule_revision=o.schedule_revision
                  AND s.next_slot_at=o.target_at AND s.next_slot_index=o.slot_index
                  AND NOT b.admin_hold AND NOT b.is_deleted
            )",
        )
        .bind(claim.book_id)
        .bind(claim.schedule_revision)
        .bind(&claim.local_date)
        .bind(claim.slot_index)
        .bind(&claim.target_utc)
        .bind(token)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        Ok(current)
    }
}
