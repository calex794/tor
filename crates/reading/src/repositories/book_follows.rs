//! Explicit book-follow persistence, separate from bookmarks and entitlements.
#![forbid(unsafe_code)]
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgBookFollows { pool: PgPool }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowError { Invalid, NotFound, Unavailable }

impl PgBookFollows {
    pub fn new(pool: PgPool) -> Self { Self { pool } }

    pub async fn follow(&self, user_id: i64, book_id: i64) -> Result<(), FollowError> {
        if user_id <= 0 || book_id <= 0 { return Err(FollowError::Invalid); }
        let found: Option<i64> = sqlx::query_scalar(
            "INSERT INTO reader_book_follows (user_id, book_id)              SELECT $1, b.id FROM books b WHERE b.id = $2              AND b.visibility = 'public' AND NOT b.admin_hold AND NOT b.is_deleted              ON CONFLICT (user_id, book_id) DO UPDATE SET book_id = EXCLUDED.book_id RETURNING book_id"
        ).bind(user_id).bind(book_id).fetch_optional(&self.pool).await
            .map_err(|_| FollowError::Unavailable)?;
        found.map(|_| ()).ok_or(FollowError::NotFound)
    }

    pub async fn unfollow(&self, user_id: i64, book_id: i64) -> Result<(), FollowError> {
        if user_id <= 0 || book_id <= 0 { return Err(FollowError::Invalid); }
        sqlx::query("DELETE FROM reader_book_follows WHERE user_id = $1 AND book_id = $2")
            .bind(user_id).bind(book_id).execute(&self.pool).await
            .map_err(|_| FollowError::Unavailable)?;
        Ok(())
    }

    pub async fn is_following(&self, user_id: i64, book_id: i64) -> Result<bool, FollowError> {
        if user_id <= 0 || book_id <= 0 { return Err(FollowError::Invalid); }
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM reader_book_follows WHERE user_id = $1 AND book_id = $2)")
            .bind(user_id).bind(book_id).fetch_one(&self.pool).await
            .map_err(|_| FollowError::Unavailable)
    }
}
