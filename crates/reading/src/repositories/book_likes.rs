//! NEwBOO book-level likes. This is not an F-05 star vote or chapter thanks.
#![forbid(unsafe_code)]

use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgBookLikes {
    pool: PgPool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LikeMutation {
    NotVisible,
    Added,
    AlreadyLiked,
    Removed,
    AlreadyRemoved,
}

#[derive(Debug, Eq, PartialEq)]
pub struct LikeSummary {
    pub title: String,
    pub count: i64,
    pub liked: bool,
}

impl PgBookLikes {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Only show counts for discoverable books. Never expose hidden/held
    /// books through a differently shaped like-counter response.
    pub async fn public_count(&self, book_id: i64) -> Result<Option<i64>, sqlx::Error> {
        if book_id <= 0 {
            return Ok(None);
        }
        sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM book_likes l
                       WHERE l.book_id=b.id AND l.stats_trusted)::BIGINT
             FROM books b WHERE b.id=$1 AND b.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted",
        )
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn visible_summary(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<Option<LikeSummary>, sqlx::Error> {
        if user_id <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title,
                    (SELECT COUNT(*) FROM book_likes l
                     WHERE l.book_id=b.id AND l.stats_trusted)::BIGINT AS count,
                    EXISTS (SELECT 1 FROM book_likes l
                            WHERE l.book_id=b.id AND l.user_id=$2) AS liked
             FROM books b WHERE b.id=$1 AND b.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted",
        )
        .bind(book_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(LikeSummary {
                title: row.try_get("title")?,
                count: row.try_get("count")?,
                liked: row.try_get("liked")?,
            })
        })
        .transpose()
    }

    /// Both public visibility and the mutation are checked in one SQL
    /// statement. A unique primary key, not an in-memory counter, protects
    /// against duplicate clicks and simultaneous devices.
    pub async fn set_like(
        &self,
        user_id: i64,
        book_id: i64,
        liked: bool,
        stats_trusted: bool,
    ) -> Result<LikeMutation, sqlx::Error> {
        if user_id <= 0 || book_id <= 0 {
            return Ok(LikeMutation::NotVisible);
        }
        let row = if liked {
            sqlx::query(
                "WITH visible AS (
                     SELECT id FROM books
                     WHERE id=$1 AND visibility='public'
                       AND NOT admin_hold AND NOT is_deleted
                 ), added AS (
                     INSERT INTO book_likes (book_id,user_id,stats_trusted)
                     SELECT id,$2,$3 FROM visible
                     ON CONFLICT (user_id,book_id) DO NOTHING
                     RETURNING book_id
                 )
                 SELECT EXISTS(SELECT 1 FROM visible) AS visible,
                        EXISTS(SELECT 1 FROM added) AS changed",
            )
            .bind(book_id)
            .bind(user_id)
            .bind(stats_trusted)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query(
                "WITH visible AS (
                     SELECT id FROM books
                     WHERE id=$1 AND visibility='public'
                       AND NOT admin_hold AND NOT is_deleted
                 ), removed AS (
                     DELETE FROM book_likes
                     WHERE book_id IN (SELECT id FROM visible) AND user_id=$2
                     RETURNING book_id
                 )
                 SELECT EXISTS(SELECT 1 FROM visible) AS visible,
                        EXISTS(SELECT 1 FROM removed) AS changed",
            )
            .bind(book_id)
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?
        };
        if !row.try_get::<bool, _>("visible")? {
            return Ok(LikeMutation::NotVisible);
        }
        match (liked, row.try_get::<bool, _>("changed")?) {
            (true, true) => Ok(LikeMutation::Added),
            (true, false) => Ok(LikeMutation::AlreadyLiked),
            (false, true) => Ok(LikeMutation::Removed),
            (false, false) => Ok(LikeMutation::AlreadyRemoved),
        }
    }
}
