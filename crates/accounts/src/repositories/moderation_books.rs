//! A-05: book moderation is independent of publication and paid reading.
//! Only a live site administrator/moderator may list books or change a hold.
use super::PgAccounts;
use sqlx::{Postgres, Transaction};

#[derive(Debug)]
pub enum ModerationBooksError {
    Denied,
    Invalid,
    Missing,
    Stale,
    Unavailable,
}

// Row shape remains private to the bounded site-moderation queue.
type ModerationBookRow = (i64, String, String, String, i64, bool, Option<i64>);

#[derive(Debug)]
pub struct ModerationBook {
    pub visibility_status: String,
    pub visibility_status_revision: i64,
    pub id: i64,
    pub title: String,
    pub visibility: String,
    pub admin_hold: bool,
    pub owner_user_id: Option<i64>,
}

pub(super) async fn current_moderator(
    tx: &mut Transaction<'_, Postgres>,
    session: &[u8; 32],
) -> Result<i64, ModerationBooksError> {
    let actor: Option<i64> = sqlx::query_scalar(
        "SELECT u.id FROM users u
         JOIN sessions s ON s.user_id=u.id
           AND s.token_hash=$1 AND s.revoked_at IS NULL
           AND s.expires_at > now()
         JOIN account_roles r ON r.user_id=u.id
           AND r.role IN ('moderator','administrator') AND r.is_active
         WHERE u.account_state='active'
         ORDER BY r.role LIMIT 1
         FOR SHARE OF u,s,r",
    )
    .bind(session.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ModerationBooksError::Unavailable)?;
    actor.ok_or(ModerationBooksError::Denied)
}

impl PgAccounts {
    /// Bounded private metadata queue. No chapter text, purchase, or media is
    /// read; role and session remain locked until the query finishes.
    pub async fn moderation_books(
        &self,
        session: &[u8; 32],
        before: Option<i64>,
    ) -> Result<Vec<ModerationBook>, ModerationBooksError> {
        if before.is_some_and(|value| value <= 0) {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        current_moderator(&mut tx, session).await?;
        let rows: Vec<ModerationBookRow> = sqlx::query_as(
            "SELECT id,title,visibility,visibility_status,visibility_status_revision,
                    admin_hold,owner_user_id
             FROM books WHERE NOT is_deleted
               AND ($1::bigint IS NULL OR id < $1)
             ORDER BY id DESC LIMIT 40",
        )
        .bind(before)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    title,
                    visibility,
                    visibility_status,
                    visibility_status_revision,
                    admin_hold,
                    owner_user_id,
                )| ModerationBook {
                    id,
                    title,
                    visibility,
                    visibility_status,
                    visibility_status_revision,
                    admin_hold,
                    owner_user_id,
                },
            )
            .collect())
    }

    /// Lock live role/session and book in the same transaction as its audit.
    /// This can only change the moderation hold; an author decides whether
    /// the book is a draft or public through the existing publisher.
    pub async fn moderate_book_hold(
        &self,
        session: &[u8; 32],
        book_id: i64,
        expected_hold: bool,
        new_hold: bool,
    ) -> Result<(), ModerationBooksError> {
        if book_id <= 0 || expected_hold == new_hold {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let actor = current_moderator(&mut tx, session).await?;
        let previous: Option<bool> = sqlx::query_scalar(
            "SELECT admin_hold FROM books WHERE id=$1 AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        let Some(previous) = previous else {
            return Err(ModerationBooksError::Missing);
        };
        if previous != expected_hold {
            return Err(ModerationBooksError::Stale);
        }
        sqlx::query("UPDATE books SET admin_hold=$2 WHERE id=$1")
            .bind(book_id)
            .bind(new_hold)
            .execute(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        sqlx::query(
            "INSERT INTO moderation_book_hold_events
             (actor_user_id,book_id,before_hold,after_hold)
             VALUES($1,$2,$3,$4)",
        )
        .bind(actor)
        .bind(book_id)
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
    /// S020: only a live site moderator/administrator may modify the
    /// NEwBOO four-state moderation visibility. Never change publication,
    /// chapter visibility, access level, rights or the independent hold.
    pub async fn moderate_book_visibility(
        &self,
        session: &[u8; 32],
        book_id: i64,
        expected_revision: i64,
        expected_status: &str,
        requested_status: &str,
    ) -> Result<i64, ModerationBooksError> {
        const STATES: [&str; 4] = ["active", "hidden", "pending_moderation", "blocked"];
        if book_id <= 0
            || expected_revision <= 0
            || !STATES.contains(&expected_status)
            || !STATES.contains(&requested_status)
            || expected_status == requested_status
        {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        // Live session + role FOR SHARE; concurrent role revocation waits.
        let actor = current_moderator(&mut tx, session).await?;
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT visibility_status,visibility_status_revision
             FROM books WHERE id=$1 AND NOT is_deleted FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        let (previous, revision) = row.ok_or(ModerationBooksError::Missing)?;
        if revision != expected_revision || previous != expected_status {
            return Err(ModerationBooksError::Stale);
        }
        // Initial approval must be an explicit immutable registration
        // decision, never the routine visibility form. An unapproved
        // Book cannot be activated through the generic endpoint.
        if previous == "pending_moderation" || requested_status == "active" {
            let required: (bool, Option<i64>) = sqlx::query_as(
                "SELECT registration_requires_review,registration_approval_decision_id
                 FROM books WHERE id=$1",
            )
            .bind(book_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
            if (previous == "pending_moderation" && required.0)
                || (requested_status == "active" && required.0 && required.1.is_none())
            {
                return Err(ModerationBooksError::Stale);
            }
        }
        let next = revision.checked_add(1).ok_or(ModerationBooksError::Stale)?;
        sqlx::query(
            "UPDATE books SET visibility_status=$2,visibility_status_revision=$3
             WHERE id=$1 AND visibility_status_revision=$4",
        )
        .bind(book_id)
        .bind(requested_status)
        .bind(next)
        .bind(revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        sqlx::query(
            "INSERT INTO moderation_book_visibility_events
             (actor_user_id,book_id,before_status,after_status,before_revision,after_revision)
             VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(actor)
        .bind(book_id)
        .bind(previous)
        .bind(requested_status)
        .bind(revision)
        .bind(next)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(next)
    }
}
