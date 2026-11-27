//! Durable Postbox correlation; no public endpoint or provider credentials.
use super::suppression::record_attempt;
use crate::domain::{
    launch::{DispatchGrant, PreparedDispatch},
    postbox::{valid_id, PostboxEvent},
    quota::identity::QuotaKeySigner,
    suppression::{AttemptRef, FeedbackDecision},
    MailClaim, MailOutcome,
};
use rand::{rngs::OsRng, RngCore};
use sqlx::{PgPool, Postgres, Transaction};
use std::time::Instant;
use tor_contracts::mail::MailPurpose;

#[derive(Clone)]
pub struct PgPostboxFeedback {
    pool: PgPool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostboxDecision {
    Feedback(FeedbackDecision),
    Duplicate,
    Uncorrelated,
    InvalidRecipient,
}

async fn guard(tx: &mut Transaction<'_, Postgres>) -> Result<Option<Vec<u8>>, sqlx::Error> {
    sqlx::query_scalar("SELECT key_binding FROM mail_send_budget_guard WHERE singleton FOR UPDATE")
        .fetch_one(&mut **tx)
        .await
}

impl PgPostboxFeedback {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Commit the correlation before SMTP. A second call must not permit
    /// another submission. Use the returned value as Original-Message-ID.
    pub async fn prepare(
        &self,
        claim: MailClaim,
        identity_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        self.prepare_inner(claim, identity_id, None, None).await
    }

    /// The caller resolves the current request, verifies recipient policy,
    /// reserves shared quota, then generates a fresh 256-bit proof in memory.
    /// Only its SHA-256 digest crosses this port. Commit the correlation and
    /// proof together, or neither; the prior charged reservation stays charged.
    pub async fn prepare_password_reset(
        &self,
        claim: MailClaim,
        identity_id: &str,
        token_hash: &[u8; 32],
    ) -> Result<Option<String>, sqlx::Error> {
        self.prepare_inner(claim, identity_id, Some(token_hash), None)
            .await
    }

    /// For non-reset dispatch, the exact grant must own the charged row.
    pub async fn prepare_dispatch(
        &self,
        grant: &DispatchGrant,
        identity_id: &str,
    ) -> Result<Option<PreparedDispatch>, sqlx::Error> {
        let started = Instant::now();
        let marker = self
            .prepare_inner(grant.claim(), identity_id, None, Some(grant))
            .await?;
        Ok(marker.map(|marker| PreparedDispatch::new(grant, marker, None, started)))
    }

    /// After authenticated SMTPS, issue the reset proof and correlation only
    /// under the same live dispatch activation and charged reservation.
    pub async fn prepare_password_reset_dispatch(
        &self,
        grant: &DispatchGrant,
        identity_id: &str,
        token_hash: &[u8; 32],
    ) -> Result<Option<PreparedDispatch>, sqlx::Error> {
        let started = Instant::now();
        let marker = self
            .prepare_inner(grant.claim(), identity_id, Some(token_hash), Some(grant))
            .await?;
        Ok(marker.map(|marker| {
            PreparedDispatch::new(grant, marker, Some(*token_hash), started)
        }))
    }

    async fn prepare_inner(
        &self,
        claim: MailClaim,
        identity_id: &str,
        reset_token_hash: Option<&[u8; 32]>,
        grant: Option<&DispatchGrant>,
    ) -> Result<Option<String>, sqlx::Error> {
        if (claim.intent.purpose == MailPurpose::PasswordReset) != reset_token_hash.is_some() {
            return Ok(None);
        }
        if !valid_id(identity_id) {
            return Err(sqlx::Error::Protocol("invalid Postbox identity".to_owned()));
        }
        let mut bytes = [0_u8; 32];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| sqlx::Error::Protocol("mail randomness unavailable".to_owned()))?;
        let marker = hex::encode(bytes);
        let mut tx = self.pool.begin().await?;
        // For reset, lock the account before the outbox/source/proof. Account
        // changes and account deletion must serialize with proof issuance.
        if reset_token_hash.is_some() {
            let active: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
            )
            .bind(claim.intent.recipient.get())
            .fetch_optional(&mut *tx)
            .await?;
            if active.is_none() {
                tx.rollback().await?;
                return Ok(None);
            }
        }
        let Some(binding) = guard(&mut tx).await? else {
            tx.rollback().await?;
            return Ok(None);
        };
        if !super::cost::submission_enabled(&mut tx, claim).await? {
            tx.rollback().await?;
            return Ok(None);
        }
        let launch_digest = if let Some(grant) = grant {
            if !super::launch::dispatch_matches(&mut tx, grant, true).await? {
                return Ok(None);
            }
            let exact_identity: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM mail_postbox_profile \
                 WHERE singleton AND identity_id=$1 AND ciphertext IS NOT NULL)",
            )
            .bind(identity_id)
            .fetch_one(&mut *tx)
            .await?;
            if !exact_identity {
                return Ok(None);
            }
            let Some(digest) = grant.digest() else {
                return Ok(None);
            };
            Some(digest.as_slice())
        } else {
            if !super::launch::legacy_allowed(&mut tx).await? {
                return Ok(None);
            }
            None
        };
        let inserted = sqlx::query(
            "INSERT INTO mail_feedback_attempts \
             (outbox_id,generation,recipient_key,key_binding,identity_id,marker,reserved_at) \
             SELECT r.outbox_id,r.generation,r.recipient_key,$7,$6,$8,r.reserved_at \
             FROM mail_send_reservations r JOIN mail_outbox m ON m.id=r.outbox_id \
               AND m.generation=r.generation \
             WHERE m.id=$1 AND m.generation=$2 AND m.purpose=$3 AND m.event_id=$4 \
               AND m.recipient_user_id=$5 AND m.status='leased' \
               AND r.launch_intent_digest IS NOT DISTINCT FROM $9 \
               AND m.lease_until>clock_timestamp() \
               AND (($9::bytea IS NULL AND m.purpose<>'password_reset') OR \
                    m.lease_until>clock_timestamp()+interval '25 seconds') \
               AND r.reserved_at>clock_timestamp()-interval '120 seconds' \
               AND r.reserved_at<=clock_timestamp() \
               AND date_trunc('month',r.reserved_at AT TIME ZONE 'Europe/Moscow') \
                 =date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow') \
               AND NOT EXISTS (SELECT 1 FROM mail_recipient_suppression s \
                 WHERE s.recipient_key=r.recipient_key \
                 AND (s.suppressed_until IS NULL OR s.suppressed_until>clock_timestamp())) \
             FOR UPDATE OF m \
             ON CONFLICT(outbox_id,generation) DO NOTHING",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(claim.intent.purpose.as_str())
        .bind(claim.intent.event_id.get())
        .bind(claim.intent.recipient.get())
        .bind(identity_id)
        .bind(&binding)
        .bind(&marker)
        .bind(launch_digest)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted != 1 {
            tx.rollback().await?;
            return Ok(None);
        }
        if let Some(token_hash) = reset_token_hash {
            // A-owned SQL port; verify the exact live source against the
            // just-prepared reservation and correlation under the same locks.
            let issued: bool = sqlx::query_scalar(
                "SELECT account_issue_password_reset_mail_proof($1,$2,$3,$4,$5)",
            )
            .bind(claim.intent.recipient.get())
            .bind(claim.intent.event_id.get())
            .bind(claim.id)
            .bind(claim.generation)
            .bind(token_hash.as_slice())
            .fetch_one(&mut *tx)
            .await?;
            if !issued {
                tx.rollback().await?;
                return Ok(None);
            }
        }
        tx.commit().await?;
        Ok(Some(format!("<tor-{marker}@pandaolate.ru>")))
    }

    /// Finalize a prepared reset attempt. On definite nonacceptance revoke
    /// only this proof. Accepted or uncertain SMTP may have delivered the
    /// message, so keep its proof live. Never release the charged quota.
    /// A receipt conflict is not permission to replay: retain the marker.
    pub async fn finish_password_reset(
        &self,
        claim: MailClaim,
        identity_id: &str,
        token_hash: &[u8; 32],
        outcome: MailOutcome,
        provider_message_id: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        if claim.intent.purpose != MailPurpose::PasswordReset || !valid_id(identity_id) {
            return Ok(false);
        }
        if outcome != MailOutcome::Accepted && provider_message_id.is_some() {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        let account: Option<i64> =
            sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR UPDATE")
                .bind(claim.intent.recipient.get())
                .fetch_optional(&mut *tx)
                .await?;
        if account.is_none() {
            return Ok(false);
        }
        let Some(binding) = guard(&mut tx).await? else {
            return Ok(false);
        };
        let updated = sqlx::query(
            "UPDATE mail_outbox m SET status=$8,lease_until=NULL,finished_at=clock_timestamp() \
             FROM mail_feedback_attempts f,password_reset_tokens p \
             WHERE m.id=$1 AND m.generation=$2 AND m.purpose='password_reset' \
               AND m.event_id=$3 AND m.recipient_user_id=$4 AND m.status='leased' \
               AND m.lease_until>clock_timestamp() \
               AND f.outbox_id=m.id AND f.generation=m.generation \
               AND f.identity_id=$5 AND f.key_binding=$6 \
               AND p.user_id=m.recipient_user_id AND p.password_reset_event_id=m.event_id \
               AND p.token_hash=$7",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(claim.intent.event_id.get())
        .bind(claim.intent.recipient.get())
        .bind(identity_id)
        .bind(&binding)
        .bind(token_hash.as_slice())
        .bind(outcome.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        if matches!(outcome, MailOutcome::Failed | MailOutcome::Cancelled) {
            sqlx::query(
                "UPDATE password_reset_tokens SET consumed_at=clock_timestamp() \
                 WHERE token_hash=$1 AND user_id=$2 AND password_reset_event_id=$3 \
                 AND consumed_at IS NULL",
            )
            .bind(token_hash.as_slice())
            .bind(claim.intent.recipient.get())
            .bind(claim.intent.event_id.get())
            .execute(&mut *tx)
            .await?;
        }
        if let Some(message_id) = provider_message_id.filter(|id| valid_id(id)) {
            // The shared guard serializes feedback and other receipt binding.
            // If another attempt owns the ID, keep the exact local marker.
            sqlx::query(
                "UPDATE mail_feedback_attempts f SET provider_message_id=$4 \
                 WHERE f.outbox_id=$1 AND f.generation=$2 AND f.identity_id=$3 \
                 AND (f.provider_message_id IS NULL OR f.provider_message_id=$4) \
                 AND NOT EXISTS (SELECT 1 FROM mail_feedback_attempts other \
                     WHERE other.identity_id=$3 AND other.provider_message_id=$4 \
                     AND (other.outbox_id,other.generation)<>(f.outbox_id,f.generation))",
            )
            .bind(claim.id)
            .bind(claim.generation)
            .bind(identity_id)
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Bind the provider ID from a successful SMTP receipt, even if the
    /// corresponding notification does not retain the original header.
    pub async fn bind_accepted_message(
        &self,
        attempt: AttemptRef,
        identity_id: &str,
        message_id: &str,
    ) -> Result<bool, sqlx::Error> {
        if !valid_id(identity_id) || !valid_id(message_id) {
            return Err(sqlx::Error::Protocol("invalid Postbox receipt".to_owned()));
        }
        let mut tx = self.pool.begin().await?;
        let Some(binding) = guard(&mut tx).await? else {
            tx.rollback().await?;
            return Ok(false);
        };
        let count = sqlx::query(
            "UPDATE mail_feedback_attempts SET provider_message_id=$4 \
             WHERE outbox_id=$1 AND generation=$2 AND identity_id=$3 AND key_binding=$5 \
               AND reserved_at>clock_timestamp()-interval '90 days' \
               AND (provider_message_id IS NULL OR provider_message_id=$4)",
        )
        .bind(attempt.outbox_id)
        .bind(attempt.generation)
        .bind(identity_id)
        .bind(message_id)
        .bind(binding)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(count == 1)
    }

    /// Caller MUST authenticate the stream/relay and restrict it to the
    /// configured Postbox identity. A parsed JSON object proves no origin.
    /// No raw event body, email address or error text is persisted.
    pub async fn record_verified_event(
        &self,
        event: &PostboxEvent,
        signer: &QuotaKeySigner,
    ) -> Result<PostboxDecision, sqlx::Error> {
        let Ok(keys) = signer.keys(&event.recipient, None) else {
            return Ok(PostboxDecision::InvalidRecipient);
        };
        let Some(reason) = event.feedback.suppression_reason() else {
            return Ok(PostboxDecision::Feedback(FeedbackDecision::Ignored));
        };
        let mut tx = self.pool.begin().await?;
        let Some(binding) = guard(&mut tx).await? else {
            tx.rollback().await?;
            return Ok(PostboxDecision::Feedback(FeedbackDecision::UnboundKey));
        };
        if binding.as_slice() != keys.key_binding() {
            tx.rollback().await?;
            return Ok(PostboxDecision::Feedback(FeedbackDecision::UnboundKey));
        }
        let row: Option<(i64, i32)> = sqlx::query_as(
            "SELECT outbox_id,generation FROM mail_feedback_attempts \
             WHERE identity_id=$1 AND recipient_key=$4 AND key_binding=$5 \
               AND reserved_at>clock_timestamp()-interval '90 days' \
               AND (provider_message_id IS NULL OR provider_message_id=$2) \
               AND ($3::text IS NULL OR marker=$3) \
               AND (provider_message_id=$2 OR marker=$3)",
        )
        .bind(&event.identity_id)
        .bind(&event.message_id)
        .bind(event.marker.as_deref())
        .bind(keys.recipient().as_slice())
        .bind(&binding)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((outbox_id, generation)) = row else {
            tx.rollback().await?;
            return Ok(PostboxDecision::Uncorrelated);
        };
        let inserted = sqlx::query(
            "INSERT INTO mail_feedback_events(identity_id,event_id,outbox_id,generation) \
             VALUES($1,$2,$3,$4) ON CONFLICT(identity_id,event_id) DO NOTHING",
        )
        .bind(&event.identity_id)
        .bind(&event.event_id)
        .bind(outbox_id)
        .bind(generation)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 0 {
            tx.rollback().await?;
            return Ok(PostboxDecision::Duplicate);
        }
        // A verified event with the exact precommitted marker can recover a
        // message ID after an ambiguous SMTP acknowledgement.
        sqlx::query(
            "UPDATE mail_feedback_attempts SET provider_message_id=$3 \
             WHERE outbox_id=$1 AND generation=$2 AND provider_message_id IS NULL",
        )
        .bind(outbox_id)
        .bind(generation)
        .bind(&event.message_id)
        .execute(&mut *tx)
        .await?;
        let decision = record_attempt(
            &mut tx,
            AttemptRef {
                outbox_id,
                generation,
            },
            reason,
        )
        .await?;
        tx.commit().await?;
        Ok(PostboxDecision::Feedback(decision))
    }

    /// Delete at most 100 expired correlations and their event-ID fences.
    pub async fn prune_expired(&self) -> Result<u64, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let _binding = guard(&mut tx).await?;
        let count = sqlx::query(
            "WITH batch AS (SELECT outbox_id,generation FROM mail_feedback_attempts \
             WHERE reserved_at<=clock_timestamp()-interval '90 days' \
             ORDER BY reserved_at,outbox_id,generation LIMIT 100 FOR UPDATE SKIP LOCKED) \
             DELETE FROM mail_feedback_attempts f USING batch b \
             WHERE f.outbox_id=b.outbox_id AND f.generation=b.generation",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(count)
    }
}
