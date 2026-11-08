//! A-owned Book mutation step-up: password verification is completed by the
//! bounded Argon2 worker before an opaque one-time proof is minted. B never
//! imports A or observes the plaintext password.
use super::postgres::PgAccounts;
use super::step_up::StepUpIssueError;
use crate::services::security::{generate_session_token, session_token_digest, VerifiedPassword};

pub const BOOK_TRANSFER_SCOPE: &str = "book.owner.transfer.v1";
pub const BOOK_SHARES_SCOPE: &str = "book.revenue.shares.v1";

impl PgAccounts {
    pub async fn issue_book_mutation_step_up(
        &self,
        actor: i64,
        session_digest: &[u8; 32],
        verified_password: &VerifiedPassword,
        book_id: i64,
        revision: i64,
        scope: &str,
        payload_digest: &[u8; 32],
    ) -> Result<String, StepUpIssueError> {
        if actor <= 0
            || book_id <= 0
            || revision <= 0
            || !matches!(scope, BOOK_TRANSFER_SCOPE | BOOK_SHARES_SCOPE)
        {
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
        // Credential rotation uses users -> sessions; use the same order.
        let hash: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users
             WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| StepUpIssueError::Unavailable)?;
        if !hash
            .as_deref()
            .is_some_and(|v| verified_password.matches_hash(v))
        {
            return Err(StepUpIssueError::StepUpRequired);
        }
        let live: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE user_id=$1 AND token_hash=$2
             AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR UPDATE",
        )
        .bind(actor)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| StepUpIssueError::Unavailable)?;
        if live != Some(actor) || !verified_password.is_fresh() {
            return Err(StepUpIssueError::StepUpRequired);
        }
        let token = generate_session_token();
        let digest = session_token_digest(&token);
        let changed = sqlx::query(
            "UPDATE sessions SET step_up_proof_hash=$3,step_up_scope=$4,
               step_up_resource_id=$5,step_up_revision=$6,step_up_payload_digest=$7,
               step_up_issued_at=statement_timestamp(),
               step_up_expires_at=statement_timestamp()+INTERVAL '5 minutes',
               step_up_used_at=NULL
             WHERE token_hash=$1 AND user_id=$2
               AND revoked_at IS NULL AND expires_at>clock_timestamp()",
        )
        .bind(session_digest.as_slice())
        .bind(actor)
        .bind(digest.as_slice())
        .bind(scope)
        .bind(book_id)
        .bind(revision)
        .bind(payload_digest.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(|_| StepUpIssueError::Unavailable)?;
        if changed.rows_affected() != 1 {
            return Err(StepUpIssueError::StepUpRequired);
        }
        tx.commit()
            .await
            .map_err(|_| StepUpIssueError::Unavailable)?;
        Ok(token)
    }
}
