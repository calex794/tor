//! D-06: trusted rejection of a pending refund request.
//! This method NEVER approves money, rewrites immutable purchases or revokes
//! paid content. Keep it off HTTP until authenticated financial moderation
//! and step-up checks are wired and independently tested.

use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectRefundRejectError {
    InvalidRequest,
    Forbidden,
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
