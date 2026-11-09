//! D-06: trusted rejection of a pending refund request.
//! This method NEVER approves money, rewrites immutable purchases or revokes
//! paid content. Financial step-up is enforced in the same transaction.

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

const REJECT_SCOPE: &str = "finance.refund.reject.v1";

pub fn rejection_step_up_payload_digest(
    actor: i64,
    request_id: i64,
    revision: i64,
    reason: &str,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(REJECT_SCOPE.as_bytes());
    digest.update(actor.to_be_bytes());
    digest.update(request_id.to_be_bytes());
    digest.update(revision.to_be_bytes());
    digest.update((reason.len() as u64).to_be_bytes());
    digest.update(reason.as_bytes());
    digest.finalize().into()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectRefundRejectError {
    InvalidRequest,
    Forbidden,
    StepUpRequired,
    InvalidStepUp,
    NotFound,
    RevisionConflict,
    Conflict,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RejectedRefundReceipt {
    pub request_id: i64,
    pub revision: i64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct PgDirectRefundDecisions {
    pool: PgPool,
}

impl PgDirectRefundDecisions {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A rejection changes only the request's terminal audit state.
    /// It is not a replacement for a full payment decision. Under the
    /// existing account model, only an active global administrator may call
    /// this private method; financial step-up is required before HTTP wiring.
    /// Same actor + same reason replays the original terminal outcome.
    pub async fn reject_once(
        &self,
        actor: i64,
        session_digest: &[u8; 32],
        step_up_token: &str,
        request_id: i64,
        expected_revision: i64,
        reason: &str,
    ) -> Result<RejectedRefundReceipt, DirectRefundRejectError> {
        let reason = reason.trim();
        if actor <= 0
            || request_id <= 0
            || expected_revision <= 0
            || reason.is_empty()
            || reason.chars().count() > 1000
            || step_up_token.len() != 43
            || !step_up_token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(DirectRefundRejectError::InvalidRequest);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| DirectRefundRejectError::Unavailable)?;
        // Lock the active administrator's role and account for the duration
        // of the rejection transaction, so revocation cannot race the write.
        let authorized: Option<i64> = sqlx::query_scalar(
            "SELECT r.user_id FROM account_roles r
             JOIN users u ON u.id=r.user_id
             WHERE r.user_id=$1 AND r.role='administrator' AND r.is_active
               AND u.account_state='active'
             FOR SHARE OF r,u",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundRejectError::Unavailable)?;
        if authorized.is_none() {
            return Err(DirectRefundRejectError::Forbidden);
        }
        // Serialize a decision with session rotation and approval step-up.
        let session: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions
             WHERE user_id=$1 AND token_hash=$2
               AND revoked_at IS NULL AND expires_at>clock_timestamp()
             FOR UPDATE",
        )
        .bind(actor)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundRejectError::Unavailable)?;
        if session.is_none() {
            return Err(DirectRefundRejectError::StepUpRequired);
        }
        let row = sqlx::query(
            "SELECT state,revision,decision_actor_id,decision_reason
             FROM finance_direct_refund_requests WHERE id=$1 FOR UPDATE",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundRejectError::Unavailable)?
        .ok_or(DirectRefundRejectError::NotFound)?;
        let state: String = row
            .try_get("state")
            .map_err(|_| DirectRefundRejectError::Unavailable)?;
        let revision: i64 = row
            .try_get("revision")
            .map_err(|_| DirectRefundRejectError::Unavailable)?;
        if state == "rejected" {
            let prior_actor: Option<i64> = row
                .try_get("decision_actor_id")
                .map_err(|_| DirectRefundRejectError::Unavailable)?;
            let prior_reason: Option<String> = row
                .try_get("decision_reason")
                .map_err(|_| DirectRefundRejectError::Unavailable)?;
            if prior_actor != Some(actor) || prior_reason.as_deref() != Some(reason) {
                return Err(DirectRefundRejectError::Conflict);
            }
            return Ok(RejectedRefundReceipt {
                request_id,
                revision,
                replayed: true,
            });
        }
        if state != "pending" {
            return Err(DirectRefundRejectError::Conflict);
        }
        if revision != expected_revision {
            return Err(DirectRefundRejectError::RevisionConflict);
        }
        // The one-use proof is bound to this decision, reason, revision and
        // *current session*. Approval-scoped tokens cannot authorize rejection.
        let proof_hash = Sha256::digest(step_up_token.as_bytes());
        let payload =
            rejection_step_up_payload_digest(actor, request_id, expected_revision, reason);
        let consumed = sqlx::query(
            "UPDATE sessions SET step_up_used_at=clock_timestamp()
             WHERE user_id=$1 AND token_hash=$2
               AND step_up_proof_hash=$3 AND step_up_scope=$4
               AND step_up_resource_id=$5 AND step_up_revision=$6
               AND step_up_payload_digest=$7
               AND step_up_issued_at<=clock_timestamp()
               AND step_up_expires_at>clock_timestamp()
               AND step_up_used_at IS NULL
               AND revoked_at IS NULL AND expires_at>clock_timestamp()",
        )
        .bind(actor)
        .bind(session_digest.as_slice())
        .bind(proof_hash.as_slice())
        .bind(REJECT_SCOPE)
        .bind(request_id)
        .bind(expected_revision)
        .bind(payload.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(|_| DirectRefundRejectError::Unavailable)?;
        if consumed.rows_affected() != 1 {
            return Err(DirectRefundRejectError::InvalidStepUp);
        }
        let decided_reference = format!("d06:reject:{request_id}");
        let new_revision: i64 = sqlx::query_scalar(
            "UPDATE finance_direct_refund_requests
             SET state='rejected',revision=revision+1,decision_actor_id=$2,
                 decision_reason=$3,decided_at=now(),decision_reference=$4
             WHERE id=$1 AND state='pending' AND revision=$5
             RETURNING revision",
        )
        .bind(request_id)
        .bind(actor)
        .bind(reason)
        .bind(decided_reference)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DirectRefundRejectError::Unavailable)?
        .ok_or(DirectRefundRejectError::RevisionConflict)?;
        tx.commit()
            .await
            .map_err(|_| DirectRefundRejectError::Unavailable)?;
        Ok(RejectedRefundReceipt {
            request_id,
            revision: new_revision,
            replayed: false,
        })
    }
}
