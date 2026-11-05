//! Reader-owned PostgreSQL bookmarks. Saving a book is NOT a chapter entitlement.
#![forbid(unsafe_code)]

use crate::services::library::{
    LibraryError, SavedBook, SavedBookPage, SavedFreeChapter, SavedFreeChapterLink,
    SavedFreeChapterPage, MAX_LIBRARY_LIMIT,
};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgLibrary {
    pool: PgPool,
}

impl PgLibrary {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn add_book(&self, user_id: i64, book_id: i64) -> Result<(), LibraryError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(LibraryError::Invalid);
        }
        // One SQL statement atomically saves the Book link AND every currently
        // released free edition, exactly as NEwBOO's add-to-library action.
        // Pin by immutable version ID. Existing pins are never upgraded to V2
        // and paid/unreleased/draft/held chapters are never granted.
        // Lock the Book row to serialize a simultaneous deletion decision.
        let saved: Option<i64> = sqlx::query_scalar(
            "WITH eligible AS MATERIALIZED ( \
                SELECT b.id FROM books AS b \
                JOIN users AS u ON u.id=$1 AND u.account_state='active' \
                WHERE b.id=$2 AND b.visibility='public' \
                  AND b.visibility_status='active' \
                  AND b.lifecycle_status<>'pending_deletion' \
                  AND NOT b.admin_hold AND NOT b.is_deleted \
                  AND (b.access_level='basic' \
                       OR (b.access_level='adult' AND u.adult_enabled) \
                       OR (b.access_level='pro_adult' AND EXISTS ( \
                           SELECT 1 FROM user_level_grants AS g \
                           WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active \
                       ))) \
                FOR SHARE OF b \
             ), saved_book AS ( \
                INSERT INTO reader_library_items (user_id, book_id) \
                SELECT $1, e.id FROM eligible AS e \
                ON CONFLICT (user_id, book_id) DO UPDATE SET book_id=EXCLUDED.book_id \
                RETURNING book_id \
             ), saved_free AS ( \
                INSERT INTO reader_saved_free_chapters (user_id, chapter_id, version_id) \
                SELECT $1, c.id, p.version_id \
                FROM saved_book AS li \
                JOIN chapters AS c ON c.book_id=li.book_id \
                JOIN chapter_publications AS p ON p.chapter_id=c.id \
                JOIN chapter_versions AS v ON v.chapter_id=c.id AND v.id=p.version_id \
                WHERE c.visibility='public' AND c.access_level='free' \
                  AND NOT c.admin_hold AND NOT c.is_deleted \
                  AND (c.reader_released_at IS NULL OR c.reader_released_at<=now()) \
                ON CONFLICT (user_id, chapter_id) DO NOTHING \
                RETURNING chapter_id \
             ) \
             SELECT li.book_id FROM saved_book AS li \
             CROSS JOIN (SELECT COUNT(*) FROM saved_free) AS pins",
        )
        .bind(user_id)
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LibraryError::Unavailable)?;
        saved.map(|_| ()).ok_or(LibraryError::NotFound)
    }

    pub async fn remove_book(&self, user_id: i64, book_id: i64) -> Result<(), LibraryError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(LibraryError::Invalid);
        }
        // Lock a saved Book link first to serialize an in-flight add, then
        // remove only the reader's free pins. Purchased versions and financial
        // records are independent, and must remain untouched by unbookmarking.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LibraryError::Unavailable)?;
        let _locked: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM reader_library_items \
             WHERE user_id=$1 AND book_id=$2 FOR UPDATE",
        )
        .bind(user_id)
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LibraryError::Unavailable)?;
        sqlx::query(
            "DELETE FROM reader_saved_free_chapters AS pins \
             USING chapters AS c \
             WHERE pins.chapter_id=c.id AND pins.user_id=$1 AND c.book_id=$2",
        )
        .bind(user_id)
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| LibraryError::Unavailable)?;
        sqlx::query("DELETE FROM reader_library_items WHERE user_id=$1 AND book_id=$2")
            .bind(user_id)
            .bind(book_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| LibraryError::Unavailable)?;
        tx.commit().await.map_err(|_| LibraryError::Unavailable)?;
        Ok(())
    }

    pub async fn list_books(
        &self,
        user_id: i64,
        limit: u16,
        after: Option<i64>,
    ) -> Result<SavedBookPage, LibraryError> {
        if user_id <= 0
            || limit == 0
            || limit > MAX_LIBRARY_LIMIT
            || after.is_some_and(|cursor| cursor <= 0)
        {
            return Err(LibraryError::Invalid);
        }
        // The user_id comes from the verified session. This is a private
        // previously-saved Book list, not public discovery; pending_deletion
        // preserves the bookmark and shows its status without granting a chapter.
        // Held, blocked, hard-deleted and age-ineligible Books stay denied.
        // Purchases/text are not joined or selected.
        let rows = sqlx::query(
            "SELECT li.id AS cursor_id, b.id AS book_id, b.title, \
                    (b.lifecycle_status='pending_deletion') AS pending_deletion \
             FROM reader_library_items AS li \
             JOIN books AS b ON b.id = li.book_id \
             JOIN users AS u ON u.id=li.user_id AND u.account_state='active' \
             WHERE li.user_id = $1 AND ($2::BIGINT IS NULL OR li.id < $2) \
               AND b.visibility = 'public' AND b.visibility_status = 'active' AND NOT b.admin_hold AND NOT b.is_deleted \
               AND (b.access_level='basic' \
                    OR (b.access_level='adult' AND u.adult_enabled) \
                    OR (b.access_level='pro_adult' AND EXISTS ( \
                        SELECT 1 FROM user_level_grants AS g \
                        WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active \
                    ))) \
             ORDER BY li.id DESC LIMIT $3",
        )
        .bind(user_id)
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LibraryError::Unavailable)?;
        let mut items = rows
            .into_iter()
            .map(|row| {
                Ok(SavedBook {
                    cursor_id: row
                        .try_get("cursor_id")
                        .map_err(|_| LibraryError::Unavailable)?,
                    book_id: row
                        .try_get("book_id")
                        .map_err(|_| LibraryError::Unavailable)?,
                    title: row
                        .try_get("title")
                        .map_err(|_| LibraryError::Unavailable)?,
                    pending_deletion: row
                        .try_get("pending_deletion")
                        .map_err(|_| LibraryError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, LibraryError>>()?;
        let has_more = items.len() > usize::from(limit);
        items.truncate(usize::from(limit));
        let next_cursor = if has_more {
            items.last().map(|item| item.cursor_id)
        } else {
            None
        };
        Ok(SavedBookPage { items, next_cursor })
    }
}

impl PgLibrary {
    /// Return ONLY the reader's immutable saved-free edition, never a
    /// current published edition inferred from a Book bookmark or read place.
    /// All account, age, administrative and legal gates live in the text query.
    pub async fn pinned_free_chapter(
        &self,
        user_id: i64,
        chapter_id: i64,
    ) -> Result<Option<SavedFreeChapter>, LibraryError> {
        if user_id <= 0 || chapter_id <= 0 {
            return Err(LibraryError::Invalid);
        }
        let row = sqlx::query(
            "SELECT b.id AS book_id, b.title AS book_title, \
                    c.id AS chapter_id, c.chapter_no, c.title AS chapter_title, \
                    saved.version_id, v.version_no, v.body_markdown \
             FROM reader_saved_free_chapters AS saved \
             JOIN chapters AS c ON c.id=saved.chapter_id \
             JOIN books AS b ON b.id=c.book_id \
             JOIN users AS viewer ON viewer.id=saved.user_id AND viewer.account_state='active' \
             JOIN chapter_versions AS v ON v.chapter_id=c.id AND v.id=saved.version_id \
             WHERE saved.user_id=$1 AND saved.chapter_id=$2 \
               AND b.visibility_status='active' \
               AND (b.visibility='public' OR b.lifecycle_status='pending_deletion') \
               AND NOT b.admin_hold AND NOT b.is_deleted \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND c.visibility='public' \
               AND (b.access_level='basic' \
                OR (b.access_level='adult' AND viewer.adult_enabled) \
                OR (b.access_level='pro_adult' AND EXISTS ( \
                    SELECT 1 FROM user_level_grants AS g \
                    WHERE g.user_id=viewer.id AND g.level='pro_adult' \
                      AND g.is_active)))",
        )
        .bind(user_id)
        .bind(chapter_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LibraryError::Unavailable)?;
        row.map(|r| {
            Ok(SavedFreeChapter {
                book_id: r
                    .try_get("book_id")
                    .map_err(|_| LibraryError::Unavailable)?,
                book_title: r
                    .try_get("book_title")
                    .map_err(|_| LibraryError::Unavailable)?,
                chapter_id: r
                    .try_get("chapter_id")
                    .map_err(|_| LibraryError::Unavailable)?,
                chapter_no: r
                    .try_get("chapter_no")
                    .map_err(|_| LibraryError::Unavailable)?,
                chapter_title: r
                    .try_get("chapter_title")
                    .map_err(|_| LibraryError::Unavailable)?,
                version_id: r
                    .try_get("version_id")
                    .map_err(|_| LibraryError::Unavailable)?,
                version_no: r
                    .try_get("version_no")
                    .map_err(|_| LibraryError::Unavailable)?,
                body_markdown: r
                    .try_get("body_markdown")
                    .map_err(|_| LibraryError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// Keyset by immutable chapter ID: bounded at 100 even for 10k saved
    /// chapters. No paid text/body or metadata from another reader is selected.
    pub async fn pinned_free_chapters(
        &self,
        user_id: i64,
        book_id: i64,
        limit: u16,
        after: Option<i64>,
    ) -> Result<SavedFreeChapterPage, LibraryError> {
        if user_id <= 0
            || book_id <= 0
            || limit == 0
            || limit > MAX_LIBRARY_LIMIT
            || after.is_some_and(|id| id <= 0)
        {
            return Err(LibraryError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT c.id AS chapter_id, c.chapter_no, c.title AS chapter_title, v.version_no \
             FROM reader_saved_free_chapters AS saved \
             JOIN chapters AS c ON c.id=saved.chapter_id \
             JOIN books AS b ON b.id=c.book_id \
             JOIN users AS viewer ON viewer.id=saved.user_id AND viewer.account_state='active' \
             JOIN chapter_versions AS v ON v.chapter_id=c.id AND v.id=saved.version_id \
             WHERE saved.user_id=$1 AND c.book_id=$2 \
               AND ($3::BIGINT IS NULL OR c.id>$3) \
               AND b.visibility_status='active' \
               AND (b.visibility='public' OR b.lifecycle_status='pending_deletion') \
               AND NOT b.admin_hold AND NOT b.is_deleted \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND c.visibility='public' \
               AND (b.access_level='basic' \
                OR (b.access_level='adult' AND viewer.adult_enabled) \
                OR (b.access_level='pro_adult' AND EXISTS ( \
                    SELECT 1 FROM user_level_grants AS g \
                    WHERE g.user_id=viewer.id AND g.level='pro_adult' \
                      AND g.is_active))) \
             ORDER BY c.id LIMIT $4",
        )
        .bind(user_id)
        .bind(book_id)
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LibraryError::Unavailable)?;
        let mut items = rows
            .into_iter()
            .map(|r| {
                Ok(SavedFreeChapterLink {
                    chapter_id: r
                        .try_get("chapter_id")
                        .map_err(|_| LibraryError::Unavailable)?,
                    chapter_no: r
                        .try_get("chapter_no")
                        .map_err(|_| LibraryError::Unavailable)?,
                    chapter_title: r
                        .try_get("chapter_title")
                        .map_err(|_| LibraryError::Unavailable)?,
                    version_no: r
                        .try_get("version_no")
                        .map_err(|_| LibraryError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, LibraryError>>()?;
        let has_more = items.len() > usize::from(limit);
        items.truncate(usize::from(limit));
        let next_cursor = has_more
            .then(|| items.last().map(|c| c.chapter_id))
            .flatten();
        Ok(SavedFreeChapterPage { items, next_cursor })
    }
}
