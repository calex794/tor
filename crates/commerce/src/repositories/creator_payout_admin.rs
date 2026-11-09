//! S039/D-09: audited administrator read and rejection of payout reservations.
//! Rejection releases a hold; it never debits a wallet or posts a payment.
#![forbid(unsafe_code)]
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayoutAdminError {
    Invalid,
    NotFound,
    Conflict,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutAdminRow {
    pub id: i64,
    pub user_id: i64,
    pub amount_minor: i64,
    pub state: String,
    pub created_utc: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayoutRejectReceipt {
    pub request_id: i64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct PgCreatorPayoutAdmin {
    pool: PgPool,
}

impl PgCreatorPayoutAdmin {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // The caller supplies only an opaque authenticated session digest. Resolve
    // the administrator in PostgreSQL and retain locks through the audit commit.
    // Role revocation, session expiration and account suspension fail closed.
    async fn active_administrator(
        tx: &mut Transaction<'_, Postgres>,
        session_digest: &[u8; 32],
    ) -> Result<i64, PayoutAdminError> {
        let admin: Option<i64> = sqlx::query_scalar(
            "SELECT u.id FROM sessions s
             JOIN users u ON u.id=s.user_id
             JOIN account_roles r ON r.user_id=u.id
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL
               AND s.expires_at>clock_timestamp()
               AND u.account_state='active'
               AND r.role='administrator' AND r.is_active
             FOR SHARE OF s,u,r",
        )
        .bind(session_digest.as_slice())
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        admin.ok_or(PayoutAdminError::NotFound)
    }

    /// A bounded, audited operator list. No email, full name, book, or private
    /// purchase details are returned. Failed audits return no financial data.
    pub async fn list(
        &self,
        session_digest: &[u8; 32],
        before: Option<i64>,
    ) -> Result<Vec<PayoutAdminRow>, PayoutAdminError> {
        if before.is_some_and(|id| id <= 0) {
            return Err(PayoutAdminError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| PayoutAdminError::Unavailable)?;
        let admin = Self::active_administrator(&mut tx, session_digest).await?;
        let rows: Vec<(i64, i64, i64, String, String)> = sqlx::query_as(
            "SELECT id,user_id,amount_minor,state,
                    to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
             FROM finance_creator_payout_requests
             WHERE ($1::bigint IS NULL OR id<$1)
             ORDER BY id DESC LIMIT 30",
        )
        .bind(before)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        sqlx::query(
            "INSERT INTO finance_creator_payout_admin_audit
             (admin_user_id,action) VALUES($1,'list_requests')",
        )
        .bind(admin)
        .execute(&mut *tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        tx.commit().await.map_err(|_| PayoutAdminError::Unavailable)?;
        Ok(rows.into_iter().map(|(id,user_id,amount_minor,state,created_utc)| {
            PayoutAdminRow {id,user_id,amount_minor,state,created_utc}
        }).collect())
    }

    /// Atomically reject one pending request and release its reservation.
    /// Same administrator and identical reason replay without another audit.
    /// Another decision or changed reason is a conflict. Never touch the
    /// balance, wallet journal or external payment systems.
    pub async fn reject_once(
        &self,
        session_digest: &[u8; 32],
        request_id: i64,
        reason: &str,
    ) -> Result<PayoutRejectReceipt, PayoutAdminError> {
        let reason = reason.trim();
        if request_id <= 0 || reason.is_empty() || reason.chars().count()>500 {
            return Err(PayoutAdminError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| PayoutAdminError::Unavailable)?;
        let admin = Self::active_administrator(&mut tx, session_digest).await?;
        let row: Option<(String, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT state,decision_actor_id,decision_reason
             FROM finance_creator_payout_requests WHERE id=$1 FOR UPDATE",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        let (state, prior_actor, prior_reason) = row.ok_or(PayoutAdminError::NotFound)?;
        if state=="rejected" {
            if prior_actor!=Some(admin) || prior_reason.as_deref()!=Some(reason) {
                return Err(PayoutAdminError::Conflict);
            }
            return Ok(PayoutRejectReceipt {request_id,replayed:true});
        }
        if state!="pending" {
            return Err(PayoutAdminError::Conflict);
        }
        sqlx::query(
            "UPDATE finance_creator_payout_requests
             SET state='rejected',decided_at=clock_timestamp(),
                 decision_actor_id=$2,decision_reason=$3
             WHERE id=$1 AND state='pending'",
        )
        .bind(request_id).bind(admin).bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        sqlx::query(
            "INSERT INTO finance_creator_payout_admin_audit
             (admin_user_id,payout_request_id,action)
             VALUES($1,$2,'reject')",
        )
        .bind(admin).bind(request_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| PayoutAdminError::Unavailable)?;
        tx.commit().await.map_err(|_| PayoutAdminError::Unavailable)?;
        Ok(PayoutRejectReceipt {request_id,replayed:false})
    }
}
