//! A-05/MOD-C01: bounded chapter metadata queue and reversible moderator hold.
//! Author publishing, paid ownership and immutable versions are separate.
use super::{
    moderation_books::{current_moderator, ModerationBooksError},
    PgAccounts,
};

#[derive(Debug, sqlx::FromRow)]
pub struct ModerationChapter {
    pub id: i64,
    pub book_id: i64,
    pub book_title: String,
    pub chapter_no: i32,
    pub title: String,
    pub visibility: String,
    pub access_level: String,
    pub admin_hold: bool,
    pub moderation_state: String,
    pub has_publication: bool,
    pub has_version: bool,
}

impl PgAccounts {
    /// A moderator sees metadata, never body_markdown or a buyer grant.
    /// Session, live role and user remain locked until the read commits.
    pub async fn moderation_chapters(
        &self,
        session: &[u8; 32],
        before: Option<i64>,
        status: &str,
    ) -> Result<Vec<ModerationChapter>, ModerationBooksError> {
        if before.is_some_and(|value| value <= 0)
            || !matches!(
                status,
                "all" | "hold" | "hidden" | "blocked" | "draft" | "published" | "no_version"
            )
        {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        current_moderator(&mut tx, session).await?;
        let rows: Vec<ModerationChapter> =
            sqlx::query_as(
                "SELECT c.id, c.book_id, b.title AS book_title, c.chapter_no, c.title,
                        c.visibility, c.access_level, c.admin_hold, c.moderation_state,
                        (p.chapter_id IS NOT NULL) AS has_publication,
                        EXISTS (SELECT 1 FROM chapter_versions v WHERE v.chapter_id=c.id)
                            AS has_version
                 FROM chapters c
                 JOIN books b ON b.id=c.book_id
                 LEFT JOIN chapter_publications p ON p.chapter_id=c.id
                 WHERE NOT c.is_deleted AND NOT b.is_deleted
                   AND ($1::bigint IS NULL OR c.id < $1)
                   AND ($2='all'
                        OR ($2='hold' AND c.admin_hold)
                        OR ($2='hidden' AND c.moderation_state='hidden')
                        OR ($2='blocked' AND c.moderation_state='blocked')
                        OR ($2='draft' AND c.visibility='draft')
                        OR ($2='published' AND c.visibility='public' AND p.chapter_id IS NOT NULL)
                        OR ($2='no_version'
                            AND NOT EXISTS (SELECT 1 FROM chapter_versions v WHERE v.chapter_id=c.id)))
                 ORDER BY c.id DESC LIMIT 40",
            )
            .bind(before)
            .bind(status)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(rows)
    }

    /// Compare-and-set is inside a transaction with live role locks.
    /// Never alter visibility, versions, prices, publication or purchases.
    pub async fn moderate_chapter_hold(
        &self,
        session: &[u8; 32],
        chapter_id: i64,
        expected_hold: bool,
        new_hold: bool,
    ) -> Result<(), ModerationBooksError> {
        if chapter_id <= 0 || expected_hold == new_hold {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let actor = current_moderator(&mut tx, session).await?;
        let previous: Option<(bool, i64)> = sqlx::query_as(
            "SELECT c.admin_hold, c.book_id
             FROM chapters c JOIN books b ON b.id=c.book_id
             WHERE c.id=$1 AND NOT c.is_deleted AND NOT b.is_deleted
             FOR UPDATE OF c",
        )
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        let Some((previous, book_id)) = previous else {
            return Err(ModerationBooksError::Missing);
        };
        if previous != expected_hold {
            return Err(ModerationBooksError::Stale);
        }
        sqlx::query(
            "UPDATE chapters SET admin_hold=$2, moderation_state=$3 WHERE id=$1",
        )
        .bind(chapter_id)
        .bind(new_hold)
        .bind(if new_hold { "hold" } else { "none" })
        .execute(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        sqlx::query(
            "INSERT INTO moderation_chapter_hold_events
                (actor_user_id,book_id,chapter_id,before_hold,after_hold)
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(actor)
        .bind(book_id)
        .bind(chapter_id)
        .bind(previous)
        .bind(new_hold)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(())
    }
    /// Preserve NEwBOO's distinct hide/block decisions without altering an
    /// author-owned draft/public state, a published version or paid rights.
    /// A changed decision is always audited, even if both states are held.
    pub async fn moderate_chapter_decision(
        &self,
        session: &[u8; 32],
        chapter_id: i64,
        expected_state: &str,
        next_state: &str,
    ) -> Result<(), ModerationBooksError> {
        fn allowed(value: &str) -> bool {
            matches!(value, "none" | "hold" | "hidden" | "blocked")
        }
        if chapter_id <= 0
            || expected_state == next_state
            || !allowed(expected_state)
            || !allowed(next_state)
        {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let actor = current_moderator(&mut tx, session).await?;
        let previous: Option<(bool, String, i64)> = sqlx::query_as(
            "SELECT c.admin_hold,c.moderation_state,c.book_id
             FROM chapters c JOIN books b ON b.id=c.book_id
             WHERE c.id=$1 AND NOT c.is_deleted AND NOT b.is_deleted
             FOR UPDATE OF c",
        )
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        let Some((previous_hold, previous_state, book_id)) = previous else {
            return Err(ModerationBooksError::Missing);
        };
        if previous_state != expected_state || previous_hold != (expected_state != "none") {
            return Err(ModerationBooksError::Stale);
        }
        let next_hold = next_state != "none";
        sqlx::query("UPDATE chapters SET admin_hold=$2, moderation_state=$3 WHERE id=$1")
            .bind(chapter_id)
            .bind(next_hold)
            .bind(next_state)
            .execute(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        sqlx::query(
            "INSERT INTO moderation_chapter_decision_events
                (actor_user_id,book_id,chapter_id,before_state,after_state)
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(actor)
        .bind(book_id)
        .bind(chapter_id)
        .bind(&previous_state)
        .bind(next_state)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        if previous_hold != next_hold {
            sqlx::query(
                "INSERT INTO moderation_chapter_hold_events
                    (actor_user_id,book_id,chapter_id,before_hold,after_hold)
                 VALUES($1,$2,$3,$4,$5)",
            )
            .bind(actor)
            .bind(book_id)
            .bind(chapter_id)
            .bind(previous_hold)
            .bind(next_hold)
            .execute(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        }
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(())
    }

}
