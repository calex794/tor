//! MOD-REG-01: registration approval is not a generic visibility edit.
//! Audit and the first approver stay server-private, never in public Book DTOs.
use super::{
    moderation_books::{current_moderator, ModerationBooksError},
    PgAccounts,
};
use sqlx::Row;

#[derive(Debug)]
pub struct RegistrationCandidate {
    pub book_id: i64,
    pub title: String,
    pub owner_user_id: Option<i64>,
    pub revision: i64,
}

#[derive(Debug)]
pub struct PrivateRegistrationApproval {
    pub reviewer_user_id: i64,
    pub reviewer_name: String,
    pub approved_unix: i64,
}

impl PgAccounts {
    /// Bounded site-role-only queue, not the entire Book catalogue.
    pub async fn moderation_registration_queue(
        &self,
        session: &[u8; 32],
        before: Option<i64>,
    ) -> Result<Vec<RegistrationCandidate>, ModerationBooksError> {
        self.moderation_registration_queue_filtered(session, before, None)
            .await
    }

    /// Search is evaluated in PostgreSQL before keyset LIMIT 40, never on a
    /// clipped HTML result set. Site authorization remains inside the query tx.
    pub async fn moderation_registration_queue_filtered(
        &self,
        session: &[u8; 32],
        before: Option<i64>,
        search: Option<&str>,
    ) -> Result<Vec<RegistrationCandidate>, ModerationBooksError> {
        if before.is_some_and(|id| id <= 0)
            || search.is_some_and(|q| q.len() > 120 || q.contains('\0'))
        {
            return Err(ModerationBooksError::Invalid);
        }
        let search = search.map(str::trim).filter(|q| !q.is_empty());
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        current_moderator(&mut tx, session).await?;
        let rows = sqlx::query(
            "SELECT id,title,owner_user_id,visibility_status_revision
             FROM books
             WHERE registration_requires_review AND visibility_status='pending_moderation'
               AND registration_approval_decision_id IS NULL AND NOT is_deleted
               AND ($1::bigint IS NULL OR id<$1)
               AND ($2::text IS NULL OR id::text=$2
                    OR position(lower($2) in lower(title))>0)
             ORDER BY id DESC LIMIT 40",
        )
        .bind(before)
        .bind(search)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        rows.into_iter()
            .map(|r| {
                Ok(RegistrationCandidate {
                    book_id: r
                        .try_get("id")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    title: r
                        .try_get("title")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    owner_user_id: r
                        .try_get("owner_user_id")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                    revision: r
                        .try_get("visibility_status_revision")
                        .map_err(|_| ModerationBooksError::Unavailable)?,
                })
            })
            .collect()
    }

    pub async fn moderation_registration_pending_count(
        &self,
        session: &[u8; 32],
    ) -> Result<i64, ModerationBooksError> {
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        current_moderator(&mut tx, session).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM books
             WHERE registration_requires_review AND visibility_status='pending_moderation'
               AND registration_approval_decision_id IS NULL AND NOT is_deleted",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(count)
    }

    /// Reviewer's site-role is re-checked under the same transaction as
    /// the Book CAS + immutable decision + legacy visibility event.
    pub async fn moderate_book_registration(
        &self,
        session: &[u8; 32],
        book_id: i64,
        expected_revision: i64,
        action: &str,
        reason: &str,
    ) -> Result<(), ModerationBooksError> {
        let reason = reason.trim();
        if book_id <= 0
            || expected_revision <= 0
            || !matches!(action, "approve" | "reject")
            || reason.len() > 2000
            || (action == "reject" && reason.is_empty())
        {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let reviewer = current_moderator(&mut tx, session).await?;
        let row = sqlx::query(
            "SELECT owner_user_id,visibility_status,visibility_status_revision,
                    registration_requires_review,registration_approval_decision_id,
                    admin_hold,is_deleted
             FROM books WHERE id=$1 FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?
        .ok_or(ModerationBooksError::Missing)?;
        let owner: Option<i64> = row
            .try_get("owner_user_id")
            .map_err(|_| ModerationBooksError::Unavailable)?;
        if owner == Some(reviewer) {
            return Err(ModerationBooksError::Denied);
        }
        let before: String = row
            .try_get("visibility_status")
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let current_revision: i64 = row
            .try_get("visibility_status_revision")
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let required: bool = row
            .try_get("registration_requires_review")
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let approved: Option<i64> = row
            .try_get("registration_approval_decision_id")
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let held: bool = row
            .try_get("admin_hold")
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let deleted: bool = row
            .try_get("is_deleted")
            .map_err(|_| ModerationBooksError::Unavailable)?;
        if before != "pending_moderation"
            || current_revision != expected_revision
            || !required
            || approved.is_some()
            || held
            || deleted
        {
            return Err(ModerationBooksError::Stale);
        }
        let next = current_revision
            .checked_add(1)
            .ok_or(ModerationBooksError::Stale)?;
        let outcome = if action == "approve" {
            "approved"
        } else {
            "rejected"
        };
        let after = if action == "approve" {
            "active"
        } else {
            "hidden"
        };
        let decision_id: i64 = sqlx::query_scalar(
            "INSERT INTO book_registration_decisions
             (book_id,reviewer_user_id,outcome,expected_visibility_revision,
              resulting_visibility_revision,reason)
             VALUES($1,$2,$3,$4,$5,$6) RETURNING id",
        )
        .bind(book_id)
        .bind(reviewer)
        .bind(outcome)
        .bind(current_revision)
        .bind(next)
        .bind(if reason.is_empty() {
            None
        } else {
            Some(reason)
        })
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        sqlx::query(
            "UPDATE books SET visibility_status=$2,visibility_status_revision=$3,
                registration_approval_decision_id=$4
             WHERE id=$1 AND visibility_status='pending_moderation'
               AND visibility_status_revision=$5",
        )
        .bind(book_id)
        .bind(after)
        .bind(next)
        .bind(if action == "approve" {
            Some(decision_id)
        } else {
            None
        })
        .bind(current_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        sqlx::query(
            "INSERT INTO moderation_book_visibility_events
             (actor_user_id,book_id,before_status,after_status,before_revision,after_revision)
             VALUES($1,$2,'pending_moderation',$3,$4,$5)",
        )
        .bind(reviewer)
        .bind(book_id)
        .bind(after)
        .bind(current_revision)
        .bind(next)
        .execute(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(())
    }

    /// A moderator can see exactly who *initially approved* this Book.
    /// Later holds, cover reviews or deletion decisions cannot overwrite it.
    pub async fn private_book_registration_approver(
        &self,
        session: &[u8; 32],
        book_id: i64,
    ) -> Result<Option<PrivateRegistrationApproval>, ModerationBooksError> {
        if book_id <= 0 {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        current_moderator(&mut tx, session).await?;
        let row = sqlx::query(
            "SELECT d.reviewer_user_id,u.display_name,
                    EXTRACT(EPOCH FROM d.decided_at)::bigint AS approved_unix
             FROM books b
             JOIN book_registration_decisions d ON d.id=b.registration_approval_decision_id
               AND d.book_id=b.id AND d.outcome='approved'
             JOIN users u ON u.id=d.reviewer_user_id
             WHERE b.id=$1",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        row.map(|r| {
            Ok(PrivateRegistrationApproval {
                reviewer_user_id: r
                    .try_get("reviewer_user_id")
                    .map_err(|_| ModerationBooksError::Unavailable)?,
                reviewer_name: r
                    .try_get("display_name")
                    .map_err(|_| ModerationBooksError::Unavailable)?,
                approved_unix: r
                    .try_get("approved_unix")
                    .map_err(|_| ModerationBooksError::Unavailable)?,
            })
        })
        .transpose()
    }
}
