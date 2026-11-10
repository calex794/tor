//! S018: future email channel permission requires a one-time proof sent to
//! the live account mailbox AND deliberate opt-in. This repository never sends.
#![forbid(unsafe_code)]
use crate::repositories::notification_preferences::PreferenceError;
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgEmailNotificationConsent {
    pool: PgPool,
}

impl PgEmailNotificationConsent {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Trusted I/G entrypoint only: caller must have authenticated the actor,
    /// generated a cryptographically random proof, and arranged bounded
    /// delivery to that actor's existing account email. Never return the proof
    /// or any address to untrusted HTTP callers.
    ///
    /// Reissuing invalidates an older challenge; even the previous active
    /// opt-in is disabled until the replacement proof is consumed.
    /// A per-user 60-second throttle rejects repeated initiation.
    pub async fn issue_proof(
        &self,
        actor: i64,
        proof_digest: &[u8; 32],
    ) -> Result<bool, PreferenceError> {
        if actor <= 0 {
            return Err(PreferenceError::Invalid);
        }
        let saved: Option<i64> = sqlx::query_scalar(
            "INSERT INTO reader_notification_email_proofs AS proof
                (user_id,email_sha256,challenge_hash,challenge_expires_at,
                 requested_at,verified_at,opted_in_at,revoked_at)
             SELECT id,sha256(convert_to(email_normalized,'UTF8')),$2,
                    statement_timestamp()+interval '30 minutes',
                    statement_timestamp(),NULL,NULL,NULL
             FROM users WHERE id=$1 AND account_state='active'
             ON CONFLICT(user_id) DO UPDATE
             SET email_sha256=EXCLUDED.email_sha256,
                 challenge_hash=EXCLUDED.challenge_hash,
                 challenge_expires_at=EXCLUDED.challenge_expires_at,
                 requested_at=EXCLUDED.requested_at,
                 verified_at=NULL,opted_in_at=NULL,revoked_at=NULL
             WHERE proof.requested_at <= statement_timestamp()-interval '60 seconds'
             RETURNING user_id",
        )
        .bind(actor)
        .bind(proof_digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(saved.is_some())
    }

    /// Atomic single-use email-address proof and explicit content opt-in.
    /// A different account, changed address, revoked account, stale token or
    /// second consumption fails closed. The email address is never copied.
    pub async fn confirm_opt_in(
        &self,
        actor: i64,
        proof_digest: &[u8; 32],
    ) -> Result<bool, PreferenceError> {
        if actor <= 0 {
            return Err(PreferenceError::Invalid);
        }
        let consumed: Option<i64> = sqlx::query_scalar(
            "UPDATE reader_notification_email_proofs p
             SET challenge_hash=NULL,challenge_expires_at=NULL,
                 verified_at=statement_timestamp(),
                 opted_in_at=statement_timestamp(),revoked_at=NULL
             FROM users u
             WHERE p.user_id=$1 AND u.id=p.user_id
               AND u.account_state='active'
               AND p.challenge_hash=$2
               AND p.challenge_expires_at>statement_timestamp()
               AND p.email_sha256=sha256(convert_to(u.email_normalized,'UTF8'))
             RETURNING p.user_id",
        )
        .bind(actor)
        .bind(proof_digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(consumed.is_some())
    }

    /// A signed-in reader may revoke at any time, including while a proof
    /// is still pending. Revocation also consumes any outstanding challenge.
    pub async fn revoke(&self, actor: i64) -> Result<bool, PreferenceError> {
        if actor <= 0 {
            return Err(PreferenceError::Invalid);
        }
        let revoked: Option<i64> = sqlx::query_scalar(
            "UPDATE reader_notification_email_proofs
             SET challenge_hash=NULL,challenge_expires_at=NULL,
                 opted_in_at=NULL,revoked_at=statement_timestamp()
             WHERE user_id=$1 RETURNING user_id",
        )
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(revoked.is_some())
    }

    /// This is an eligibility gate for future send-intent creation, NOT a
    /// send authorization. The future sender must repeat live chapter ACL,
    /// address and channel verification immediately before external handoff.
    /// A saved cadence alone never constitutes transport consent.
    pub async fn email_eligible(&self, actor: i64) -> Result<bool, PreferenceError> {
        if actor <= 0 {
            return Err(PreferenceError::Invalid);
        }
        let eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM reader_notification_email_proofs p
                JOIN users u ON u.id=p.user_id
                JOIN reader_notification_delivery_preferences d
                  ON d.user_id=p.user_id
                WHERE p.user_id=$1 AND u.account_state='active'
                  AND p.verified_at IS NOT NULL AND p.opted_in_at IS NOT NULL
                  AND p.revoked_at IS NULL
                  AND p.email_sha256=sha256(convert_to(u.email_normalized,'UTF8'))
                  AND d.delivery_mode IN ('immediate','smart_time','daily_digest')
             )",
        )
        .bind(actor)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(eligible)
    }
}

fn unavailable(_: sqlx::Error) -> PreferenceError {
    PreferenceError::Unavailable
}
