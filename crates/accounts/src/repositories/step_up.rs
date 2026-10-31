//! A-owned administrator step-up: verified password -> one session-bound proof.
//! No commerce repository, financial mutation or request ownership lookup here.

use super::postgres::PgAccounts;
use crate::services::security::{
    generate_session_token, session_token_digest, VerifiedPassword,
};
use sqlx::Row;

const REFUND_SCOPE: &str = "finance.refund.approve.v1";
const MAX_STEP_UP_SECONDS: f64 = 300.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepUpIssueError {
    InvalidIntent,
    Forbidden,
    StepUpRequired,
    Unavailable,
}

impl PgAccounts {
    /// Authorizes only the specified decision digest. The caller must
    /// construct this digest from the exact pending financial operation.
    /// A one-time proof is stored only as SHA-256 on the active session.
    pub async fn issue_administrator_refund_step_up(
        &self,
        actor: i64,
        session_digest: &[u8; 32],
        verified_password: &VerifiedPassword,
        request_id: i64,
        expected_revision: i64,
        decision_digest: &[u8; 32],
    ) -> Result<String, StepUpIssueError> {
        if actor <= 0 || request_id <= 0 || expected_revision <= 0 {
            return Err(StepUpIssueError::InvalidIntent);
        }
        if !verified_password.is_fresh() {
            return Err(StepUpIssueError::StepUpRequired);
        }

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| StepUpIssueError::Unavailable)?;
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT r.user_id FROM account_roles r
             JOIN users u ON u.id=r.user_id
             WHERE r.user_id=$1 AND r.role='administrator' AND r.is_active
               AND u.account_state='active'
             FOR SHARE OF r,u",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| StepUpIssueError::Unavailable)?;
        if allowed.is_none() {
            return Err(StepUpIssueError::Forbidden);
        }

        let session = sqlx::query(
            "SELECT u.password_hash FROM sessions s
             JOIN users u ON u.id=s.user_id
             WHERE s.user_id=$1 AND s.token_hash=$2
               AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp()
               AND u.account_state='active'
             FOR UPDATE OF s",
        )
        .bind(actor)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| StepUpIssueError::Unavailable)?
        .ok_or(StepUpIssueError::StepUpRequired)?;
        let stored_hash: String = session
            .try_get("password_hash")
            .map_err(|_| StepUpIssueError::Unavailable)?;
        if !verified_password.matches_hash(&stored_hash) {
            return Err(StepUpIssueError::StepUpRequired);
        }

        let token = generate_session_token();
        let token_hash = session_token_digest(&token);
        let updated = sqlx::query(
            "UPDATE sessions
             SET step_up_proof_hash=$3, step_up_scope=$4,
                 step_up_resource_id=$5, step_up_revision=$6,
                 step_up_payload_digest=$7,
                 step_up_issued_at=statement_timestamp(),
                 step_up_expires_at=statement_timestamp()
                     + ($8::double precision * INTERVAL '1 second'),
                 step_up_used_at=NULL
             WHERE token_hash=$1 AND user_id=$2
               AND revoked_at IS NULL AND expires_at>clock_timestamp()",
        )
        .bind(session_digest.as_slice())
        .bind(actor)
        .bind(token_hash.as_slice())
        .bind(REFUND_SCOPE)
        .bind(request_id)
        .bind(expected_revision)
        .bind(decision_digest.as_slice())
        .bind(MAX_STEP_UP_SECONDS)
        .execute(&mut *tx)
        .await
        .map_err(|_| StepUpIssueError::Unavailable)?;
        if updated.rows_affected() != 1 {
            return Err(StepUpIssueError::StepUpRequired);
        }
        tx.commit()
            .await
            .map_err(|_| StepUpIssueError::Unavailable)?;
        Ok(token)
    }
}
