//! BR-060: a deleted Book keeps the actor's immutable free V1 cursor.
//! No bookmark, cursor, public chapter or purchased record is an entitlement.
#![forbid(unsafe_code)]

use super::{parse_receipt, parse_stored, PgReadingSync};
use crate::services::{
    progress::{reader_blocks, valid_text_place, ReadingPlace},
    sync::{checked_place, StoredProgress, SyncError, SyncReceipt},
};
use sqlx::Row;

impl PgReadingSync {
    /// Return the per-Book revision only after checking this exact free pin,
    /// current session, Book age level, and moderation/hard-delete status.
    pub async fn archived_free_state(
        &self,
        actor: i64,
        chapter_id: i64,
        version_id: i64,
        session_digest: &[u8; 32],
    ) -> Result<Option<StoredProgress>, SyncError> {
        if actor <= 0 || chapter_id <= 0 || version_id <= 0 {
            return Err(SyncError::Invalid);
        }
        let row = sqlx::query(
            r#"SELECT progress.chapter_id, progress.version_id,
                      progress.block_index, progress.char_offset, progress.revision,
                      to_char(progress.updated_at AT TIME ZONE 'UTC',
                        'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at
               FROM reader_saved_free_chapters pin
               JOIN chapters chapter ON chapter.id=pin.chapter_id
               JOIN books book ON book.id=chapter.book_id
               JOIN users viewer ON viewer.id=pin.user_id AND viewer.account_state='active'
               JOIN sessions active_session
                 ON active_session.user_id=viewer.id
                AND active_session.token_hash=$4::bytea
                AND active_session.revoked_at IS NULL
                AND active_session.expires_at>now()
               JOIN chapter_versions edition
                 ON edition.id=pin.version_id AND edition.chapter_id=chapter.id
               LEFT JOIN reader_book_positions progress
                 ON progress.user_id=viewer.id AND progress.book_id=book.id
               WHERE pin.user_id=$1 AND pin.chapter_id=$2 AND pin.version_id=$3
                 AND book.lifecycle_status='ordinary_deleted'
                 AND book.ordinary_deleted_at IS NOT NULL
                 AND book.visibility='draft' AND book.visibility_status='active'
                 AND NOT book.admin_hold AND NOT book.is_deleted
                 AND (book.access_level='basic'
                   OR (book.access_level='adult' AND viewer.adult_enabled)
                   OR (book.access_level='pro_adult' AND EXISTS (
                     SELECT 1 FROM user_level_grants age_grant
                     WHERE age_grant.user_id=viewer.id
                       AND age_grant.level='pro_adult' AND age_grant.is_active)))
                 AND chapter.visibility='public' AND chapter.access_level='free'
                 AND NOT chapter.admin_hold AND NOT chapter.is_deleted
                 AND (chapter.reader_released_at IS NULL
                      OR chapter.reader_released_at<=now())"#,
        )
        .bind(actor)
        .bind(chapter_id)
        .bind(version_id)
        .bind(session_digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| SyncError::Unavailable)?
        .ok_or(SyncError::NotFound)?;

        // A cursor for a *different* chapter or edition may determine the CAS
        // revision, but the web response must not render its old position.
        if row
            .try_get::<Option<i64>, _>("chapter_id")
            .map_err(|_| SyncError::Unavailable)?
            .is_none()
        {
            return Ok(None);
        }
        parse_stored(row).map(Some)
    }

    /// Advance only an explicitly pinned historical free edition. This
    /// deliberately does not mint completion, ranking, financial or outbox facts.
    /// The existing one-row-per-Book CAS remains authoritative across devices.
    pub async fn save_archived_free(
        &self,
        actor: i64,
        chapter_id: i64,
        version_id: i64,
        place: ReadingPlace,
        expected_revision: i64,
        session_digest: &[u8; 32],
    ) -> Result<SyncReceipt, SyncError> {
        if actor <= 0 || chapter_id <= 0 || version_id <= 0 {
            return Err(SyncError::Invalid);
        }
        checked_place(place, expected_revision)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| SyncError::Unavailable)?;
        // Hold a consistent access decision until the position is committed.
        // Concurrent pin removal, Book blocking or session revocation must
        // serialize with this operation rather than authorizing a stale write.
        let edition = sqlx::query(
            r#"SELECT chapter.book_id, chapter.chapter_no, chapter.edit_revision,
                      edition.body_markdown
               FROM reader_saved_free_chapters pin
               JOIN chapters chapter ON chapter.id=pin.chapter_id
               JOIN books book ON book.id=chapter.book_id
               JOIN users viewer ON viewer.id=pin.user_id AND viewer.account_state='active'
               JOIN sessions active_session
                 ON active_session.user_id=viewer.id
                AND active_session.token_hash=$4::bytea
                AND active_session.revoked_at IS NULL
                AND active_session.expires_at>now()
               JOIN chapter_versions edition
                 ON edition.id=pin.version_id AND edition.chapter_id=chapter.id
               WHERE pin.user_id=$1 AND pin.chapter_id=$2 AND pin.version_id=$3
                 AND book.lifecycle_status='ordinary_deleted'
                 AND book.ordinary_deleted_at IS NOT NULL
                 AND book.visibility='draft' AND book.visibility_status='active'
                 AND NOT book.admin_hold AND NOT book.is_deleted
                 AND (book.access_level='basic'
                   OR (book.access_level='adult' AND viewer.adult_enabled)
                   OR (book.access_level='pro_adult' AND EXISTS (
                      SELECT 1 FROM user_level_grants age_grant
                      WHERE age_grant.user_id=viewer.id
                        AND age_grant.level='pro_adult' AND age_grant.is_active)))
                 AND chapter.visibility='public' AND chapter.access_level='free'
                 AND NOT chapter.admin_hold AND NOT chapter.is_deleted
                 AND (chapter.reader_released_at IS NULL
                      OR chapter.reader_released_at<=now())
               FOR SHARE OF pin,chapter,book,viewer,active_session,edition"#,
        )
        .bind(actor)
        .bind(chapter_id)
        .bind(version_id)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)?
        .ok_or(SyncError::NotFound)?;

        let body: String = edition
            .try_get("body_markdown")
            .map_err(|_| SyncError::Unavailable)?;
        if !crate::domain::renderer::valid_reader_length(&body) {
            return Err(SyncError::Invalid);
        }
        if valid_text_place(place, &reader_blocks(&body)).is_err() {
            return Err(SyncError::Invalid);
        }
        let book_id: i64 = edition
            .try_get("book_id")
            .map_err(|_| SyncError::Unavailable)?;
        let chapter_no: i32 = edition
            .try_get("chapter_no")
            .map_err(|_| SyncError::Unavailable)?;
        let chapter_revision: i64 = edition
            .try_get("edit_revision")
            .map_err(|_| SyncError::Unavailable)?;

        let row = sqlx::query(
            r#"INSERT INTO reader_book_positions AS saved
                   (user_id,book_id,chapter_id,chapter_no,chapter_revision,
                    version_id,block_index,char_offset,revision)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,1)
               ON CONFLICT (user_id,book_id) DO UPDATE SET
                   chapter_id=EXCLUDED.chapter_id,
                   chapter_no=EXCLUDED.chapter_no,
                   chapter_revision=EXCLUDED.chapter_revision,
                   version_id=EXCLUDED.version_id,
                   block_index=EXCLUDED.block_index,
                   char_offset=EXCLUDED.char_offset,
                   revision=saved.revision+1,
                   updated_at=now()
               WHERE saved.revision=$9 AND (
                 saved.chapter_no<EXCLUDED.chapter_no OR (
                   saved.chapter_no=EXCLUDED.chapter_no
                   AND saved.chapter_id=EXCLUDED.chapter_id AND (
                     saved.version_id<>EXCLUDED.version_id
                     OR saved.block_index<EXCLUDED.block_index
                     OR (saved.block_index=EXCLUDED.block_index
                         AND saved.char_offset<EXCLUDED.char_offset)
                   )
                 )
               )
               RETURNING revision,
                 to_char(updated_at AT TIME ZONE 'UTC',
                    'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at"#,
        )
        .bind(actor)
        .bind(book_id)
        .bind(chapter_id)
        .bind(chapter_no)
        .bind(chapter_revision)
        .bind(version_id)
        .bind(place.block_index)
        .bind(place.char_offset)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SyncError::Unavailable)?
        .ok_or(SyncError::Conflict)?;

        let receipt = parse_receipt(row)?;
        tx.commit().await.map_err(|_| SyncError::Unavailable)?;
        Ok(receipt)
    }
}
