//! NEwBOO chapter thanks: one confirmed gift-free appreciation per reader+chapter.
//! No payment, completion, rating or purchase rights are created by this action.
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgChapterThanks {
    pool: PgPool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ThanksSummary {
    pub book_id: i64,
    pub title: String,
    pub count: i64,
    pub already_thanked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThanksMutation {
    NotVisible,
    Added,
    Repeated,
}

impl PgChapterThanks {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fail closed for hidden/held/deleted/unreleased/paid chapters. Paid
    /// entitlements are not inferred from direct purchase facts here.
    pub async fn visible_free_summary(
        &self,
        user_id: i64,
        chapter_id: i64,
    ) -> Result<Option<ThanksSummary>, sqlx::Error> {
        if user_id <= 0 || chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT c.book_id, c.title,
                    (SELECT COUNT(*) FROM chapter_thanks t WHERE t.chapter_id=c.id) AS count,
                    EXISTS (SELECT 1 FROM chapter_thanks t
                            WHERE t.chapter_id=c.id AND t.user_id=$2) AS already_thanked
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.id=$1 AND b.visibility='public'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND c.visibility='public' AND c.access_level='free'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())",
        )
        .bind(chapter_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(ThanksSummary {
                book_id: row.try_get("book_id")?,
                title: row.try_get("title")?,
                count: row.try_get("count")?,
                already_thanked: row.try_get("already_thanked")?,
            })
        })
        .transpose()
    }

    /// Single SQL statement enforces visibility and idempotency atomically,
    /// including simultaneous retries from two devices. The primary key,
    /// not an in-process lock, is authoritative.
    pub async fn thank_visible_free(
        &self,
        user_id: i64,
        chapter_id: i64,
    ) -> Result<ThanksMutation, sqlx::Error> {
        if user_id <= 0 || chapter_id <= 0 {
            return Ok(ThanksMutation::NotVisible);
        }
        let row = sqlx::query(
            "WITH visible AS (
                 SELECT c.book_id
                 FROM chapters c JOIN books b ON b.id=c.book_id
                 JOIN chapter_publications p ON p.chapter_id=c.id
                 WHERE c.id=$1 AND b.visibility='public'
                   AND NOT b.admin_hold AND NOT b.is_deleted
                   AND c.visibility='public' AND c.access_level='free'
                   AND NOT c.admin_hold AND NOT c.is_deleted
                   AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
             ), inserted AS (
                 INSERT INTO chapter_thanks (user_id,chapter_id,book_id)
                 SELECT $2,$1,book_id FROM visible
                 ON CONFLICT (user_id,chapter_id) DO NOTHING
                 RETURNING chapter_id
             )
             SELECT EXISTS(SELECT 1 FROM visible) AS visible,
                    EXISTS(SELECT 1 FROM inserted) AS inserted",
        )
        .bind(chapter_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        if !row.try_get::<bool, _>("visible")? {
            return Ok(ThanksMutation::NotVisible);
        }
        if row.try_get::<bool, _>("inserted")? {
            Ok(ThanksMutation::Added)
        } else {
            Ok(ThanksMutation::Repeated)
        }
    }
}
