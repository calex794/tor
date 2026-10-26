//! PostgreSQL-backed private continuation positions. No purchase rights inferred.
#![forbid(unsafe_code)]
use crate::services::progress::{ContinueReading, ProgressError, ReadingPlace};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgReadingProgress {
    pool: PgPool,
}

impl PgReadingProgress {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn saved_place(
        &self,
        user_id: i64,
        chapter_id: i64,
        version_id: i64,
    ) -> Result<Option<ReadingPlace>, ProgressError> {
        let row = sqlx::query(
            "SELECT block_index, char_offset FROM reader_book_positions \
             WHERE user_id = $1 AND chapter_id = $2 AND version_id = $3",
        )
        .bind(user_id)
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| ProgressError::Unavailable)?;
        row.map(|record| {
            Ok(ReadingPlace {
                block_index: record
                    .try_get("block_index")
                    .map_err(|_| ProgressError::Unavailable)?,
                char_offset: record
                    .try_get("char_offset")
                    .map_err(|_| ProgressError::Unavailable)?,
            })
        })
        .transpose()
    }

    pub async fn continue_reading(
        &self,
        user_id: i64,
    ) -> Result<Vec<ContinueReading>, ProgressError> {
        // Always check current book/chapter visibility; a saved position grants no access.
        // A different published version invalidates the old paragraph anchor.
        let rows = sqlx::query(
            "SELECT b.id AS book_id, b.title AS book_title, c.id AS chapter_id, \
                    c.chapter_no, c.title AS chapter_title, \
                    CASE WHEN progress.version_id = publication.version_id \
                         THEN progress.block_index ELSE 0 END AS block_index \
             FROM reader_book_positions AS progress \
             JOIN books AS b ON b.id = progress.book_id \
             JOIN chapters AS c ON c.id = progress.chapter_id AND c.book_id = b.id \
             JOIN chapter_publications AS publication ON publication.chapter_id = c.id \
             JOIN chapter_versions AS v \
               ON v.id = publication.version_id AND v.chapter_id = c.id \
             WHERE progress.user_id = $1 AND b.visibility = 'public' \
               AND NOT b.admin_hold AND NOT b.is_deleted \
               AND c.visibility = 'public' AND c.access_level = 'free' \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now()) \
             ORDER BY progress.updated_at DESC, b.id DESC LIMIT 30",
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
        let result = sqlx::query(
            "INSERT INTO reader_book_positions AS saved \
                (user_id, book_id, chapter_id, chapter_no, chapter_revision, \
                 version_id, block_index, char_offset) \
             SELECT $1, c.book_id, c.id, c.chapter_no, c.edit_revision, $3, $4, $5 \
             FROM chapters AS c \
             JOIN books AS b ON b.id = c.book_id \
             JOIN chapter_publications AS published ON published.chapter_id = c.id \
             JOIN chapter_versions AS v \
                 ON v.id = published.version_id AND v.chapter_id = c.id \
             WHERE c.id = $2 AND published.version_id = $3 \
               AND b.visibility = 'public' AND NOT b.admin_hold AND NOT b.is_deleted \
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
        .execute(&self.pool)
        .await
        .map_err(|_| ProgressError::Unavailable)?;
        if result.rows_affected() > 0 {
            return Ok(());
        }
        // Zero rows can also mean a stale/no-op command. Recheck the access gate.
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT published.version_id FROM chapters c \
             JOIN books b ON b.id = c.book_id \
             JOIN chapter_publications published ON published.chapter_id = c.id \
             WHERE c.id = $1 AND b.visibility = 'public' \
               AND NOT b.admin_hold AND NOT b.is_deleted \
               AND c.visibility = 'public' AND c.access_level = 'free' \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now())",
        )
        .bind(chapter_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| ProgressError::Unavailable)?;
        match current {
            None => Err(ProgressError::NotFound),
            Some(id) if id != version_id => Err(ProgressError::StaleVersion),
            Some(_) => Ok(()), // Delayed request is harmless; do not roll back.
        }
    }
}
