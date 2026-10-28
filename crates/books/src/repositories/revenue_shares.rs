//! B-10: one B-owned atomic schedule mutation. No per-member share PATCH.
//!
//! Shares are distinct from Book role/ACL and from D's actual earnings.
//! The Book FOR UPDATE lock is the same lock held by S030 owner transfer;
//! D checkout holds FOR SHARE on the Book through settlement.
use crate::domain::shares::{validate_schedule, RevenueShareError, ShareAssignment};
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgRevenueShares {
    pool: PgPool,
}

impl PgRevenueShares {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn replace_schedule(
        &self,
        actor_id: i64,
        book_id: i64,
        expected_revision: i64,
        assignments: &[ShareAssignment],
        reason: &str,
    ) -> Result<i64, RevenueShareError> {
        let shares = validate_schedule(assignments)?;
        if actor_id <= 0
            || book_id <= 0
            || expected_revision < 1
            || reason.is_empty()
            || reason.trim() != reason
            || reason.chars().count() > 160
        {
            return Err(RevenueShareError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| RevenueShareError::Unavailable)?;

        let owned: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND EXISTS (
                 SELECT 1 FROM book_members m
                 WHERE m.book_id=b.id AND m.user_id=$2
                   AND m.is_active AND m.role='owner'
               )
             FOR UPDATE OF b",
        )
        .bind(book_id)
        .bind(actor_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| RevenueShareError::Unavailable)?;
        if owned != Some(book_id) {
            return Err(RevenueShareError::Forbidden);
        }

        let current: Option<i64> = sqlx::query_scalar(
            "SELECT current_revision FROM revenue_share_states
             WHERE book_id=$1 FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| RevenueShareError::Unavailable)?;
        let current = current.ok_or(RevenueShareError::Missing)?;
        if current != expected_revision {
            return Err(RevenueShareError::Conflict);
        }

        let recipients: Vec<i64> = shares.iter().map(|s| s.user_id).collect();
        let eligible: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM book_members
             WHERE book_id=$1 AND is_active AND user_id=ANY($2)",
        )
        .bind(book_id)
        .bind(&recipients)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| RevenueShareError::Unavailable)?;
        if eligible != shares.len() as i64 {
            return Err(RevenueShareError::InactiveRecipient);
        }

        let next = current.checked_add(1).ok_or(RevenueShareError::Invalid)?;
        sqlx::query(
            "INSERT INTO revenue_share_versions
             (book_id,revision,changed_by_user_id,reason)
             VALUES($1,$2,$3,$4)",
        )
        .bind(book_id)
        .bind(next)
        .bind(actor_id)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(|_| RevenueShareError::Unavailable)?;

        for assignment in &shares {
            sqlx::query(
                "INSERT INTO revenue_share_assignments
                 (book_id,revision,user_id,share_percent)
                 VALUES($1,$2,$3,$4)",
            )
            .bind(book_id)
            .bind(next)
            .bind(assignment.user_id)
            .bind(assignment.share_percent)
            .execute(&mut *tx)
            .await
            .map_err(|_| RevenueShareError::Unavailable)?;
        }

        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE revenue_share_states SET current_revision=$2,updated_at=now()
             WHERE book_id=$1 AND current_revision=$3
             RETURNING current_revision",
        )
        .bind(book_id)
        .bind(next)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| RevenueShareError::Unavailable)?;
        if updated != Some(next) {
            return Err(RevenueShareError::Conflict);
        }
        tx.commit()
            .await
            .map_err(|_| RevenueShareError::Unavailable)?;
        Ok(next)
    }
}
