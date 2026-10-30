//! E-05: creator subscriptions with separate historical periods.
//! A creator is the current, B-verified owner of a public Book; a credit
//! string on a cover is not proof of account identity.
#![forbid(unsafe_code)]

use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgCreatorFollows {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreatorFollowError {
    Invalid,
    NotFound,
    Forbidden,
    Unavailable,
}

impl From<sqlx::Error> for CreatorFollowError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowedCreator {
    pub period_id: i64,
    pub creator_user_id: i64,
    pub display_name: String,
    pub started_day: String,
    pub source_book: Option<(i64, String)>,
}

impl PgCreatorFollows {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn creator_for_public_book(
        &self,
        book_id: i64,
    ) -> Result<Option<i64>, CreatorFollowError> {
        if book_id <= 0 {
            return Err(CreatorFollowError::Invalid);
        }
        Ok(sqlx::query_scalar(
            "SELECT b.owner_user_id FROM books AS b
             WHERE b.id=$1 AND b.owner_user_id IS NOT NULL
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND EXISTS (
                   SELECT 1 FROM book_members AS m WHERE m.book_id=b.id
                     AND m.user_id=b.owner_user_id AND m.role='owner' AND m.is_active
               )",
        )
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn is_following(
        &self,
        follower: i64,
        creator: i64,
    ) -> Result<bool, CreatorFollowError> {
        if follower <= 0 || creator <= 0 {
            return Err(CreatorFollowError::Invalid);
        }
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM creator_follow_periods
                WHERE follower_user_id=$1 AND creator_user_id=$2 AND ended_at IS NULL
             )",
        )
        .bind(follower)
        .bind(creator)
        .fetch_one(&self.pool)
        .await?)
    }

    /// One POST either starts a new period or preserves the original start
    /// date on retry. Actor identity is provided by the authenticated web tier.
    pub async fn follow_from_book(
        &self,
        follower: i64,
        book_id: i64,
    ) -> Result<i64, CreatorFollowError> {
        if follower <= 0 || book_id <= 0 {
            return Err(CreatorFollowError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        // Serializing the reader's own follow commands also fences rapid
        // follow/unfollow/re-follow requests across browsers.
        let actor: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(follower)
            .fetch_optional(&mut *tx)
            .await?;
        if actor.is_none() {
            return Err(CreatorFollowError::Invalid);
        }
        let creator: Option<i64> = sqlx::query_scalar(
            "SELECT b.owner_user_id FROM books AS b
             WHERE b.id=$1 AND b.owner_user_id IS NOT NULL
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND EXISTS (
                   SELECT 1 FROM book_members AS m WHERE m.book_id=b.id
                     AND m.user_id=b.owner_user_id AND m.role='owner' AND m.is_active
               )
             FOR SHARE OF b",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(creator) = creator else {
            return Err(CreatorFollowError::NotFound);
        };
        if follower == creator {
            return Err(CreatorFollowError::Forbidden);
        }
        sqlx::query(
            "INSERT INTO creator_follow_periods
                (follower_user_id,creator_user_id,source_book_id,source_kind)
             VALUES ($1,$2,$3,'book_page')
             ON CONFLICT (follower_user_id,creator_user_id) WHERE ended_at IS NULL
             DO NOTHING",
        )
        .bind(follower)
        .bind(creator)
        .bind(book_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(creator)
    }

    /// Unfollow by creator id, not current owner of an old source Book:
    /// a transferred/hidden/removed source must not strand the follower.
    pub async fn unfollow(&self, follower: i64, creator: i64) -> Result<(), CreatorFollowError> {
        if follower <= 0 || creator <= 0 {
            return Err(CreatorFollowError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let actor: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(follower)
            .fetch_optional(&mut *tx)
            .await?;
        if actor.is_none() {
            return Err(CreatorFollowError::Invalid);
        }
        sqlx::query(
            "UPDATE creator_follow_periods
             SET ended_at=clock_timestamp()
             WHERE follower_user_id=$1 AND creator_user_id=$2 AND ended_at IS NULL",
        )
        .bind(follower)
        .bind(creator)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Keyset paged, actor-private list. The historical source Book is shown
    /// only if it is still public; an old hidden Book never leaks its title.
    pub async fn list_active(
        &self,
        follower: i64,
        after: Option<i64>,
    ) -> Result<Vec<FollowedCreator>, CreatorFollowError> {
        if follower <= 0 || after.is_some_and(|cursor| cursor <= 0) {
            return Err(CreatorFollowError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT p.id AS period_id, p.creator_user_id,
                    u.display_name,
                    to_char(p.started_at AT TIME ZONE 'UTC','YYYY-MM-DD') AS started_day,
                    b.id AS source_book_id,b.title AS source_book_title
             FROM creator_follow_periods AS p
             JOIN users AS u ON u.id=p.creator_user_id
             LEFT JOIN books AS b ON b.id=p.source_book_id
                  AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
             WHERE p.follower_user_id=$1 AND p.ended_at IS NULL
                 AND ($2::bigint IS NULL OR p.id<$2)
             ORDER BY p.id DESC LIMIT 21",
        )
        .bind(follower)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let mut creators = Vec::with_capacity(rows.len());
        for row in rows {
            let book_id: Option<i64> = row.try_get("source_book_id")?;
            let book_title: Option<String> = row.try_get("source_book_title")?;
            creators.push(FollowedCreator {
                period_id: row.try_get("period_id")?,
                creator_user_id: row.try_get("creator_user_id")?,
                display_name: row.try_get("display_name")?,
                started_day: row.try_get("started_day")?,
                source_book: book_id.zip(book_title),
            });
        }
        Ok(creators)
    }
}
