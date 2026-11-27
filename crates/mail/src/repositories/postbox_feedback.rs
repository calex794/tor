//! Durable Postbox correlation; no public endpoint or provider credentials.
use super::suppression::record_attempt;
use crate::domain::{
    postbox::{valid_id, PostboxEvent},
    quota::identity::QuotaKeySigner,
    suppression::{AttemptRef, FeedbackDecision},
    MailClaim,
};
use rand::{rngs::OsRng, RngCore};
use sqlx::{PgPool, Postgres, Transaction};
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
        self.prepare_inner(claim, identity_id, None).await
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
        self.prepare_inner(claim, identity_id, Some(token_hash))
            .await
    }

    async fn prepare_inner(
        &self,
        claim: MailClaim,
        identity_id: &str,
        reset_token_hash: Option<&[u8; 32]>,
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
        let Some(binding) = guard(&mut tx).await? else {
            tx.rollback().await?;
            return Ok(None);
        };
        if !super::cost::submission_enabled(&mut tx, claim).await? {
            tx.rollback().await?;
            return Ok(None);
        }
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
        let inserted = sqlx::query(
            "INSERT INTO mail_feedback_attempts \
             (outbox_id,generation,recipient_key,key_binding,identity_id,marker,reserved_at) \
             SELECT r.outbox_id,r.generation,r.recipient_key,$7,$6,$8,r.reserved_at \
             FROM mail_send_reservations r JOIN mail_outbox m ON m.id=r.outbox_id \
               AND m.generation=r.generation \
             WHERE m.id=$1 AND m.generation=$2 AND m.purpose=$3 AND m.event_id=$4 \
               AND m.recipient_user_id=$5 AND m.status='leased' \
               AND m.lease_until>clock_timestamp() \
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
