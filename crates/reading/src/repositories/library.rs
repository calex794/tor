//! Reader-owned PostgreSQL bookmarks. Saving a book is NOT a chapter entitlement.
#![forbid(unsafe_code)]

use crate::services::library::{LibraryError, SavedBook, SavedBookPage, MAX_LIBRARY_LIMIT};
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
        // Visibility is checked inside the actual write, not in a prior page GET.
        // An existing bookmark keeps its place; a replay cannot create a duplicate.
        let saved: Option<i64> = sqlx::query_scalar(
            "INSERT INTO reader_library_items (user_id, book_id) \
             SELECT $1, b.id FROM books AS b \
             WHERE b.id = $2 AND b.visibility = 'public' \
               AND NOT b.admin_hold AND NOT b.is_deleted \
             ON CONFLICT (user_id, book_id) DO UPDATE SET book_id = EXCLUDED.book_id \
             RETURNING id",
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
        // Removal is permitted after a book becomes private or is moderated.
        sqlx::query("DELETE FROM reader_library_items WHERE user_id = $1 AND book_id = $2")
            .bind(user_id)
            .bind(book_id)
            .execute(&self.pool)
            .await
            .map_err(|_| LibraryError::Unavailable)?;
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
        // The user_id comes from the verified session. Paid text and purchases
        // are not joined or selected, and hidden books disappear from the view.
        let rows = sqlx::query(
            "SELECT li.id AS cursor_id, b.id AS book_id, b.title \
             FROM reader_library_items AS li \
             JOIN books AS b ON b.id = li.book_id \
             WHERE li.user_id = $1 AND ($2::BIGINT IS NULL OR li.id < $2) \
               AND b.visibility = 'public' AND NOT b.admin_hold AND NOT b.is_deleted \
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
