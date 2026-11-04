//! Explicit book-follow persistence, separate from bookmarks and entitlements.
#![forbid(unsafe_code)]
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgBookFollows {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowError {
    Invalid,
    NotFound,
    Unavailable,
}

impl PgBookFollows {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn follow(&self, user_id: i64, book_id: i64) -> Result<(), FollowError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(FollowError::Invalid);
        }
        // A followed Book is private intent, not a reading entitlement. Even so,
        // following by a known numeric ID must not bypass live Book age rules.
        // Never delete existing follows on age revocation: unfollow stays usable.
        let found: Option<i64> = sqlx::query_scalar(
            "INSERT INTO reader_book_follows (user_id, book_id)
             SELECT $1, b.id
             FROM books AS b
             JOIN users AS viewer ON viewer.id=$1 AND viewer.account_state='active'
             WHERE b.id=$2
               AND b.visibility='public' AND b.visibility_status='active'
               AND b.lifecycle_status<>'pending_deletion'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (
                   b.access_level='basic'
                   OR (b.access_level='adult' AND viewer.adult_enabled)
                   OR (b.access_level='pro_adult' AND EXISTS (
                       SELECT 1 FROM user_level_grants AS age
                       WHERE age.user_id=viewer.id AND age.level='pro_adult'
                         AND age.is_active
                   ))
                   OR EXISTS (
                       SELECT 1 FROM account_roles AS role
                       WHERE role.user_id=viewer.id AND role.role='administrator'
                         AND role.is_active
                   )
               )
             ON CONFLICT (user_id, book_id)
             DO UPDATE SET book_id=EXCLUDED.book_id
             RETURNING book_id",
        )
        .bind(user_id)
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| FollowError::Unavailable)?;
        found.map(|_| ()).ok_or(FollowError::NotFound)
    }

    pub async fn unfollow(&self, user_id: i64, book_id: i64) -> Result<(), FollowError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(FollowError::Invalid);
        }
        sqlx::query("DELETE FROM reader_book_follows WHERE user_id = $1 AND book_id = $2")
            .bind(user_id)
            .bind(book_id)
            .execute(&self.pool)
            .await
            .map_err(|_| FollowError::Unavailable)?;
        Ok(())
    }

    pub async fn is_following(&self, user_id: i64, book_id: i64) -> Result<bool, FollowError> {
        if user_id <= 0 || book_id <= 0 {
            return Err(FollowError::Invalid);
        }
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM reader_book_follows WHERE user_id = $1 AND book_id = $2)",
        )
        .bind(user_id)
        .bind(book_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| FollowError::Unavailable)
    }
}
