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

    /// Anonymous discovery counter: only indexable BASIC books are visible.
    /// No page or bookmark can make this a reading entitlement.
    pub async fn public_count(&self, book_id: i64) -> Result<Option<i64>, sqlx::Error> {
        self.count_for_view(book_id, false).await
    }

    /// The direct Book page is distinct from anonymous discovery. NEwBOO keeps
    /// an otherwise readable Book directly viewable during pending_deletion.
    /// This method is only for an already-authorized direct Book detail route.
    pub async fn direct_view_count(&self, book_id: i64) -> Result<Option<i64>, sqlx::Error> {
        self.count_for_view(book_id, true).await
    }

    async fn count_for_view(
        &self,
        book_id: i64,
        allow_pending_deletion: bool,
    ) -> Result<Option<i64>, sqlx::Error> {
        if book_id <= 0 {
            return Ok(None);
        }
        sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM book_likes l WHERE l.book_id=b.id)::BIGINT
             FROM books b WHERE b.id=$1 AND b.visibility='public' AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.lifecycle_status<>'pending_deletion' OR $2::boolean)
               AND b.access_level='basic'",
        )
        .bind(book_id)
        .bind(allow_pending_deletion)
        .fetch_optional(&self.pool)
        .await
    }

    /// Read the title and current like state under the same live actor age gate.
    /// The public action and existing like page still exclude pending deletion.
    pub async fn visible_summary(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<Option<LikeSummary>, sqlx::Error> {
        self.summary_for_view(user_id, book_id, false).await
    }

    /// Age-authorized direct Book detail only; this does not re-enable likes
    /// or make a pending-deletion Book publicly discoverable.
    pub async fn direct_view_summary(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<Option<LikeSummary>, sqlx::Error> {
        self.summary_for_view(user_id, book_id, true).await
    }

    async fn summary_for_view(
        &self,
        user_id: i64,
        book_id: i64,
        allow_pending_deletion: bool,
    ) -> Result<Option<LikeSummary>, sqlx::Error> {
        if user_id <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title,
                    (SELECT COUNT(*) FROM book_likes l WHERE l.book_id=b.id)::BIGINT AS count,
                    EXISTS (SELECT 1 FROM book_likes l
                            WHERE l.book_id=b.id AND l.user_id=$2) AS liked
             FROM books b
             JOIN users viewer ON viewer.id=$2 AND viewer.account_state='active'
             WHERE b.id=$1 AND b.visibility='public' AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND (b.lifecycle_status<>'pending_deletion' OR $3::boolean)
               AND (b.access_level='basic'
                 OR (b.access_level='adult' AND viewer.adult_enabled)
                 OR (b.access_level='pro_adult' AND EXISTS (
                     SELECT 1 FROM user_level_grants g
                     WHERE g.user_id=viewer.id AND g.level='pro_adult' AND g.is_active))
                 OR EXISTS (
                     SELECT 1 FROM account_roles role
                     WHERE role.user_id=viewer.id AND role.role='administrator' AND role.is_active))",
        )
        .bind(book_id)
        .bind(user_id)
        .bind(allow_pending_deletion)
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

    /// Live age rights and public visibility are checked in the same SQL
    /// mutation; a saved like never becomes an age access grant.
    /// Both public visibility and the mutation are checked in one SQL
    /// statement. A unique primary key, not an in-memory counter, protects
    /// against duplicate clicks and simultaneous devices.
    pub async fn set_like(
        &self,
        user_id: i64,
        book_id: i64,
        liked: bool,
    ) -> Result<LikeMutation, sqlx::Error> {
        if user_id <= 0 || book_id <= 0 {
            return Ok(LikeMutation::NotVisible);
        }
        let row = if liked {
            sqlx::query(
                "WITH visible AS (
                     SELECT b.id FROM books b
                     JOIN users viewer ON viewer.id=$2 AND viewer.account_state='active'
                     WHERE b.id=$1 AND b.visibility='public' AND b.visibility_status='active'
                       AND NOT b.admin_hold AND NOT b.is_deleted AND b.lifecycle_status<>'pending_deletion'
                       AND (b.access_level='basic'
                         OR (b.access_level='adult' AND viewer.adult_enabled)
                         OR (b.access_level='pro_adult' AND EXISTS (
                             SELECT 1 FROM user_level_grants g
                             WHERE g.user_id=viewer.id AND g.level='pro_adult' AND g.is_active))
                         OR EXISTS (
                             SELECT 1 FROM account_roles role
                             WHERE role.user_id=viewer.id AND role.role='administrator' AND role.is_active))
                 ), added AS (
                     INSERT INTO book_likes (book_id,user_id)
                     SELECT id,$2 FROM visible
                     ON CONFLICT (user_id,book_id) DO NOTHING
                     RETURNING book_id
                 )
                 SELECT EXISTS(SELECT 1 FROM visible) AS visible,
                        EXISTS(SELECT 1 FROM added) AS changed",
            )
            .bind(book_id)
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query(
                "WITH visible AS (
                     SELECT b.id FROM books b
                     JOIN users viewer ON viewer.id=$2 AND viewer.account_state='active'
                     WHERE b.id=$1 AND b.visibility='public' AND b.visibility_status='active'
                       AND NOT b.admin_hold AND NOT b.is_deleted AND b.lifecycle_status<>'pending_deletion'
                       AND (b.access_level='basic'
                         OR (b.access_level='adult' AND viewer.adult_enabled)
                         OR (b.access_level='pro_adult' AND EXISTS (
                             SELECT 1 FROM user_level_grants g
                             WHERE g.user_id=viewer.id AND g.level='pro_adult' AND g.is_active))
                         OR EXISTS (
                             SELECT 1 FROM account_roles role
                             WHERE role.user_id=viewer.id AND role.role='administrator' AND role.is_active))
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
