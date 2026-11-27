pub mod cost;
pub mod dispatch;
pub mod launch;
pub mod postbox_feedback;
pub mod postbox_profile;
pub mod quota;
pub mod suppression;

use crate::domain::{MailClaim, MailOutcome};

mod scheduling;
use sqlx::{PgPool, Row};
use tor_contracts::{
    auth::UserId,
    mail::{MailIntent, MailPurpose},
    paid_delivery::PositiveId,
};

#[derive(Clone)]
pub struct PgMailQueue {
    pool: PgPool,
}

impl PgMailQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The uniqueness constraint makes repeated trusted events idempotent.
    /// Source events must be durable before invoking this adapter.
    pub async fn enqueue(&self, intent: MailIntent) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "INSERT INTO mail_outbox (purpose, event_id, recipient_user_id) \
             VALUES ($1,$2,$3) ON CONFLICT (purpose,event_id,recipient_user_id) DO NOTHING",
        )
        .bind(intent.purpose.as_str())
        .bind(intent.event_id.get())
        .bind(intent.recipient.get())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Short, atomic claim. Never hold a database transaction during SMTP.
    /// News is lower priority than account and support messages.
    pub async fn claim_next(&self) -> Result<Option<MailClaim>, sqlx::Error> {
        self.claim_next_for(&[
            MailPurpose::RegistrationConfirmation,
            MailPurpose::PasswordReset,
            MailPurpose::SupportReply,
            MailPurpose::ProjectNews,
        ])
        .await
    }

    /// The common worker claims only purposes with an installed source
    /// resolver. Unimplemented product flows are not silently discarded.
    pub async fn claim_next_for(
        &self,
        purposes: &[MailPurpose],
    ) -> Result<Option<MailClaim>, sqlx::Error> {
        if purposes.is_empty() {
            return Ok(None);
        }
        let purposes: Vec<String> = purposes.iter().map(|p| p.as_str().to_owned()).collect();
        let row = sqlx::query(
            "WITH picked AS (SELECT id FROM mail_outbox \
               WHERE purpose=ANY($1) AND (status='pending' OR \
                 (status='deferred' AND next_attempt_at<=clock_timestamp())) \
               ORDER BY (purpose='project_news'), id LIMIT 1 FOR UPDATE SKIP LOCKED) \
             UPDATE mail_outbox m SET status='leased', generation=m.generation+1, \
               next_attempt_at=NULL,lease_until=clock_timestamp()+interval '120 seconds' \
               FROM picked WHERE m.id=picked.id \
             RETURNING m.id,m.generation,m.purpose,m.event_id,m.recipient_user_id",
        )
        .bind(&purposes)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            let purpose: String = r.try_get("purpose")?;
            let invalid = || sqlx::Error::Protocol("invalid mail queue reference".to_owned());
            Ok(MailClaim {
                id: r.try_get("id")?,
                generation: r.try_get("generation")?,
                intent: MailIntent {
                    purpose: MailPurpose::parse(&purpose).ok_or_else(invalid)?,
                    event_id: PositiveId::new(r.try_get("event_id")?).map_err(|_| invalid())?,
                    recipient: UserId::new(r.try_get("recipient_user_id")?)
                        .map_err(|_| invalid())?,
                },
            })
        })
        .transpose()
    }

    /// Reject a stale or already completed claim. No automatic SMTP replay.
    pub async fn finish(
        &self,
        claim: MailClaim,
        outcome: MailOutcome,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE mail_outbox SET status=$3, lease_until=NULL, finished_at=clock_timestamp() \
             WHERE id=$1 AND generation=$2 AND purpose=$4 AND event_id=$5 AND recipient_user_id=$6 \
               AND status='leased' AND lease_until>clock_timestamp()",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(outcome.as_str())
        .bind(claim.intent.purpose.as_str())
        .bind(claim.intent.event_id.get())
        .bind(claim.intent.recipient.get())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// A dead worker might have sent its message before losing its connection.
    /// Quarantine expired claims, rather than silently risking duplicate mail.
    pub async fn recover_expired(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "WITH expired AS (SELECT id FROM mail_outbox WHERE status='leased' \
               AND lease_until<=clock_timestamp() ORDER BY lease_until,id LIMIT 100 FOR UPDATE SKIP LOCKED) \
             UPDATE mail_outbox m SET status='uncertain', lease_until=NULL, finished_at=clock_timestamp() \
             FROM expired WHERE m.id=expired.id",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}
