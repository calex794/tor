//! AUTH-READ-01b: owner-scoped policy persistence, not reader authorization.
//! No HTTP route or rollout switch. The SQL activation fence refuses
//! login_preview until every serving path can enforce the restriction.
use super::PgAuthoring;
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};
use tor_contracts::guest_reading::GuestReadingPolicy;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagedGuestReading {
    pub policy: GuestReadingPolicy,
    pub revision: i64,
}

type Stored = (String, i32, i16, i64);

fn decode(row: Stored) -> Result<ManagedGuestReading, AuthorError> {
    if row.3 < 0 {
        return Err(AuthorError::Unavailable);
    }
    Ok(ManagedGuestReading {
        policy: GuestReadingPolicy::from_db(&row.0, row.1, row.2)
            .map_err(|_| AuthorError::Unavailable)?,
        revision: row.3,
    })
}

async fn lock_session(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
) -> Result<i64, AuthorError> {
    let actor: i64 = sqlx::query_scalar("SELECT user_id FROM sessions WHERE token_hash=$1")
        .bind(digest.as_slice())
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Forbidden)?;
    // A: account first, then session, matching password rotation/revocation.
    let account: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE")
            .bind(actor)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
    if account.is_none() {
        return Err(AuthorError::Forbidden);
    }
    let live: Option<i64> = sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE token_hash=$1 AND user_id=$2
           AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE",
    )
    .bind(digest.as_slice())
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    live.ok_or(AuthorError::Forbidden)
}

impl PgAuthoring {
    pub async fn manageable_guest_reading(
        &self,
        digest: &[u8; 32],
        book_id: i64,
    ) -> Result<Option<ManagedGuestReading>, AuthorError> {
        if book_id <= 0 {
            return Ok(None);
        }
        let row: Option<Stored> = sqlx::query_as(
            "SELECT b.guest_read_mode,b.guest_open_through_chapter_no,
                    b.guest_preview_percent,b.guest_policy_revision
             FROM books b
             JOIN book_members m ON m.book_id=b.id AND m.user_id=b.owner_user_id
             JOIN users u ON u.id=m.user_id
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$2
             WHERE b.id=$1 AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.visibility_status='active'
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
               AND m.role='owner' AND m.is_active AND u.account_state='active'
               AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp()",
        )
        .bind(book_id)
        .bind(digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(decode).transpose()
    }

    /// Inactive repository command. Callers must never mount this before
    /// the reader, SEO and rollout fences are accepted. CSRF/Origin belong
    /// to the future HTTP adapter. No chapter or financial write is performed.
    pub async fn replace_guest_reading(
        &self,
        digest: &[u8; 32],
        book_id: i64,
        expected_revision: i64,
        requested: GuestReadingPolicy,
    ) -> Result<ManagedGuestReading, AuthorError> {
        if book_id <= 0 || expected_revision < 0 {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let actor = lock_session(&mut tx, digest).await?;
        // B: Book before membership, matching owner transfer and revocation.
        let row: Stored = sqlx::query_as(
            "SELECT guest_read_mode,guest_open_through_chapter_no,
                    guest_preview_percent,guest_policy_revision
             FROM books WHERE id=$1 AND owner_user_id=$2
               AND NOT admin_hold AND NOT is_deleted AND visibility_status='active'
               AND lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
             FOR UPDATE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM book_members WHERE book_id=$1 AND user_id=$2
               AND role='owner' AND is_active FOR SHARE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if owner.is_none() {
            return Err(AuthorError::Missing);
        }
        let current = decode(row)?;
        if current.revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        if current.policy == requested {
            tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
            return Ok(current);
        }
        let revision = expected_revision
            .checked_add(1)
            .ok_or(AuthorError::Conflict)?;
        let result = sqlx::query(
            "UPDATE books SET guest_read_mode=$3,guest_open_through_chapter_no=$4,
                    guest_preview_percent=$5,guest_policy_revision=$6
             WHERE id=$1 AND guest_policy_revision=$2
               AND EXISTS(SELECT 1 FROM sessions WHERE token_hash=$7 AND user_id=$8
                           AND revoked_at IS NULL AND expires_at>clock_timestamp())",
        )
        .bind(book_id)
        .bind(expected_revision)
        .bind(requested.mode())
        .bind(requested.open_through())
        .bind(requested.preview_percent())
        .bind(revision)
        .bind(digest.as_slice())
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if error.as_database_error().and_then(|db| db.constraint())
                == Some("books_guest_read_rollout_closed")
            {
                AuthorError::Forbidden
            } else {
                AuthorError::Unavailable
            }
        })?;
        if result.rows_affected() != 1 {
            return Err(AuthorError::Conflict);
        }
        sqlx::query(
            "INSERT INTO book_guest_read_policy_events
                 (book_id,revision,actor_id,old_mode,new_mode,old_open_through,
                  new_open_through,old_preview_percent,new_preview_percent)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(book_id)
        .bind(revision)
        .bind(actor)
        .bind(current.policy.mode())
        .bind(requested.mode())
        .bind(current.policy.open_through())
        .bind(requested.open_through())
        .bind(current.policy.preview_percent())
        .bind(requested.preview_percent())
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(ManagedGuestReading {
            policy: requested,
            revision,
        })
    }
}
