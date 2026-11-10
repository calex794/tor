//! PostgreSQL-backed private continuation positions. No purchase rights inferred.
#![forbid(unsafe_code)]
use crate::services::progress::{ContinueReading, ProgressError, ReadingPlace, SavedPlaceState};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgReadingProgress {
    pool: PgPool,
    activity_collection: bool,
    book_return_collection: bool,
}

impl PgReadingProgress {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            activity_collection: false,
            book_return_collection: false,
        }
    }

    pub fn with_activity_collection(mut self, enabled: bool) -> Self {
        self.activity_collection = enabled;
        self
    }

    pub fn with_book_return_collection(mut self, enabled: bool) -> Self {
        self.book_return_collection = enabled;
        self
    }

    pub async fn saved_place(
        &self,
        user_id: i64,
        chapter_id: i64,
        version_id: i64,
    ) -> Result<Option<ReadingPlace>, ProgressError> {
        match self.saved_state(user_id, chapter_id, version_id).await? {
            SavedPlaceState::Current(place) => Ok(Some(place)),
            SavedPlaceState::NotSaved | SavedPlaceState::PreviousVersion => Ok(None),
        }
    }

    pub async fn saved_state(
        &self,
        user_id: i64,
        chapter_id: i64,
        version_id: i64,
    ) -> Result<SavedPlaceState, ProgressError> {
        let row = sqlx::query(
            "SELECT saved.version_id, saved.block_index, saved.char_offset \
             FROM reader_book_positions AS saved \
             JOIN chapters AS c ON c.id=saved.chapter_id \
             JOIN books AS b ON b.id=c.book_id \
             JOIN users AS viewer ON viewer.id=saved.user_id AND viewer.account_state='active' \
             WHERE saved.user_id=$1 AND saved.chapter_id=$2 \
               AND b.visibility='public' AND b.visibility_status='active' \
               AND NOT b.admin_hold AND NOT b.is_deleted \
               AND (b.access_level='basic' \
               OR (b.access_level='adult' AND viewer.adult_enabled) \
               OR (b.access_level='pro_adult' AND EXISTS ( \
                   SELECT 1 FROM user_level_grants AS age_grant \
                   WHERE age_grant.user_id=viewer.id AND age_grant.level='pro_adult' AND age_grant.is_active))) \
               AND c.visibility='public' AND c.access_level='free' \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())",
        )
        .bind(user_id)
        .bind(chapter_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| ProgressError::Unavailable)?;
        let Some(row) = row else {
            return Ok(SavedPlaceState::NotSaved);
        };
        let saved_version: i64 = row
            .try_get("version_id")
            .map_err(|_| ProgressError::Unavailable)?;
        if saved_version != version_id {
            return Ok(SavedPlaceState::PreviousVersion);
        }
        Ok(SavedPlaceState::Current(ReadingPlace {
            block_index: row
                .try_get("block_index")
                .map_err(|_| ProgressError::Unavailable)?,
            char_offset: row
                .try_get("char_offset")
                .map_err(|_| ProgressError::Unavailable)?,
        }))
    }

    pub async fn continue_reading(
        &self,
        user_id: i64,
    ) -> Result<Vec<ContinueReading>, ProgressError> {
        // Never infer an entitlement from a cursor. A finalized ordinary-deleted
        // Book may resume only an actor-owned immutable saved-free edition;
        // public Books continue to use the currently published version.
        let rows = sqlx::query(
            r#"SELECT b.id AS book_id, b.title AS book_title, c.id AS chapter_id,
                      c.chapter_no, c.title AS chapter_title,
                      (b.lifecycle_status='ordinary_deleted') AS archived_saved,
                      CASE
                        WHEN b.lifecycle_status='ordinary_deleted' THEN progress.block_index
                        WHEN progress.version_id=publication.version_id THEN progress.block_index
                        ELSE 0
                      END AS block_index
               FROM reader_book_positions AS progress
               JOIN books AS b ON b.id=progress.book_id
               JOIN users AS viewer ON viewer.id=progress.user_id AND viewer.account_state='active'
               JOIN chapters AS c ON c.id=progress.chapter_id AND c.book_id=b.id
               LEFT JOIN chapter_publications AS publication ON publication.chapter_id=c.id
               LEFT JOIN reader_saved_free_chapters AS pin
                 ON pin.user_id=progress.user_id AND pin.chapter_id=c.id
                    AND pin.version_id=progress.version_id
               JOIN chapter_versions AS edition
                 ON edition.id=progress.version_id AND edition.chapter_id=c.id
               WHERE progress.user_id=$1
                 AND (
                   (b.visibility='public' AND b.lifecycle_status<>'ordinary_deleted'
                    AND publication.version_id IS NOT NULL)
                   OR (b.lifecycle_status='ordinary_deleted'
                       AND b.ordinary_deleted_at IS NOT NULL AND pin.version_id IS NOT NULL)
                 )
                 AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
                 AND (b.access_level='basic'
                   OR (b.access_level='adult' AND viewer.adult_enabled)
                   OR (b.access_level='pro_adult' AND EXISTS (
                       SELECT 1 FROM user_level_grants AS age_grant
                       WHERE age_grant.user_id=viewer.id AND age_grant.level='pro_adult'
                         AND age_grant.is_active)))
                 AND c.visibility='public' AND c.access_level='free'
                 AND NOT c.admin_hold AND NOT c.is_deleted
                 AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               ORDER BY progress.updated_at DESC, b.id DESC LIMIT 30"#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| ProgressError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                Ok(ContinueReading {
                    book_id: row
                        .try_get("book_id")
                        .map_err(|_| ProgressError::Unavailable)?,
                    book_title: row
                        .try_get("book_title")
                        .map_err(|_| ProgressError::Unavailable)?,
                    chapter_id: row
                        .try_get("chapter_id")
                        .map_err(|_| ProgressError::Unavailable)?,
                    chapter_no: row
                        .try_get("chapter_no")
                        .map_err(|_| ProgressError::Unavailable)?,
                    chapter_title: row
                        .try_get("chapter_title")
                        .map_err(|_| ProgressError::Unavailable)?,
                    archived_saved: row
                        .try_get("archived_saved")
                        .map_err(|_| ProgressError::Unavailable)?,
                    block_index: row
                        .try_get("block_index")
                        .map_err(|_| ProgressError::Unavailable)?,
                })
            })
            .collect()
    }

    /// Advance position, or reset its anchor when the published version changes.
    /// Even two concurrent PostgreSQL writes cannot move the cursor backwards.
    pub async fn save_position(
        &self,
        user_id: i64,
        chapter_id: i64,
        version_id: i64,
        place: ReadingPlace,
    ) -> Result<(), ProgressError> {
        if user_id <= 0
            || chapter_id <= 0
            || version_id <= 0
            || !(1..=200).contains(&place.block_index)
            || !(0..=100_000).contains(&place.char_offset)
        {
            return Err(ProgressError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| ProgressError::Unavailable)?;
        // The manual form must satisfy the same C text/access boundary as the
        // CAS path. HTTP validation alone cannot authorize a trusted day fact.
        let edition = sqlx::query(
            "SELECT c.book_id,published.version_id,v.body_markdown
             FROM chapters c JOIN books b ON b.id=c.book_id
             JOIN users viewer ON viewer.id=$2 AND viewer.account_state='active'
             JOIN chapter_publications published ON published.chapter_id=c.id
             JOIN chapter_versions v ON v.id=published.version_id AND v.chapter_id=c.id
             WHERE c.id=$1 AND b.visibility='public' AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.access_level='basic'
                 OR (b.access_level='adult' AND viewer.adult_enabled)
                 OR (b.access_level='pro_adult' AND EXISTS(
                   SELECT 1 FROM user_level_grants g
                   WHERE g.user_id=viewer.id AND g.level='pro_adult' AND g.is_active)))
               AND c.visibility='public' AND c.access_level='free'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
             FOR SHARE OF c,b,viewer,published,v",
        )
        .bind(chapter_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ProgressError::Unavailable)?
        .ok_or(ProgressError::NotFound)?;
        if edition
            .try_get::<i64, _>("version_id")
            .map_err(|_| ProgressError::Unavailable)?
            != version_id
        {
            return Err(ProgressError::StaleVersion);
        }
        let body: String = edition
            .try_get("body_markdown")
            .map_err(|_| ProgressError::Unavailable)?;
        if (self.activity_collection || self.book_return_collection)
            && (!crate::domain::renderer::valid_reader_length(&body)
                || crate::services::progress::valid_text_place(
                    place,
                    &crate::services::progress::reader_blocks(&body),
                )
                .is_err())
        {
            return Err(ProgressError::Invalid);
        }
        let book: i64 = edition
            .try_get("book_id")
            .map_err(|_| ProgressError::Unavailable)?;
        let result = sqlx::query(
            "INSERT INTO reader_book_positions AS saved \
                (user_id, book_id, chapter_id, chapter_no, chapter_revision, \
                 version_id, block_index, char_offset) \
             SELECT $1, c.book_id, c.id, c.chapter_no, c.edit_revision, $3, $4, $5 \
             FROM chapters AS c \
             JOIN books AS b ON b.id = c.book_id \
             JOIN users AS viewer ON viewer.id=$1 AND viewer.account_state='active' \
             JOIN chapter_publications AS published ON published.chapter_id = c.id \
             JOIN chapter_versions AS v \
                 ON v.id = published.version_id AND v.chapter_id = c.id \
             WHERE c.id = $2 AND published.version_id = $3 \
               AND b.visibility = 'public' AND b.visibility_status = 'active' AND NOT b.admin_hold AND NOT b.is_deleted \
               AND (b.access_level='basic' \
               OR (b.access_level='adult' AND viewer.adult_enabled) \
               OR (b.access_level='pro_adult' AND EXISTS ( \
                   SELECT 1 FROM user_level_grants AS age_grant \
                   WHERE age_grant.user_id=viewer.id AND age_grant.level='pro_adult' AND age_grant.is_active))) \
               AND c.visibility = 'public' AND c.access_level = 'free' \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now()) \
             ON CONFLICT (user_id, book_id) DO UPDATE SET \
               chapter_id = EXCLUDED.chapter_id, \
               chapter_no = EXCLUDED.chapter_no, \
               chapter_revision = EXCLUDED.chapter_revision, \
               version_id = EXCLUDED.version_id, \
               block_index = EXCLUDED.block_index, \
               char_offset = EXCLUDED.char_offset, \
               revision = saved.revision + 1, updated_at = now() \
             WHERE saved.chapter_no < EXCLUDED.chapter_no \
                OR (saved.chapter_no = EXCLUDED.chapter_no \
                  AND (saved.chapter_revision < EXCLUDED.chapter_revision \
                    OR (saved.chapter_revision = EXCLUDED.chapter_revision \
                      AND (saved.version_id <> EXCLUDED.version_id \
                        OR saved.block_index < EXCLUDED.block_index \
                        OR (saved.block_index = EXCLUDED.block_index \
                            AND saved.char_offset < EXCLUDED.char_offset)))))",
        )
        .bind(user_id)
        .bind(chapter_id)
        .bind(version_id)
        .bind(place.block_index)
        .bind(place.char_offset)
        .execute(&mut *tx)
        .await
        .map_err(|_| ProgressError::Unavailable)?;
        if result.rows_affected() > 0 {
            super::returns::record(
                &mut tx,
                user_id,
                book,
                place,
                self.activity_collection,
                self.book_return_collection,
            )
            .await
            .map_err(|_| ProgressError::Unavailable)?;
        }
        // A no-op preserves the old form's harmless delayed-request behavior;
        // it cannot enroll a reader or manufacture another observed day.
        tx.commit().await.map_err(|_| ProgressError::Unavailable)?;
        Ok(())
    }
}
