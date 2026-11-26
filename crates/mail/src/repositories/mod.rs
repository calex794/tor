use crate::domain::{MailClaim, MailOutcome};
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
        let row = sqlx::query(
            "WITH picked AS (SELECT id FROM mail_outbox WHERE status='pending' \
               ORDER BY (purpose='project_news'), id LIMIT 1 FOR UPDATE SKIP LOCKED) \
             UPDATE mail_outbox m SET status='leased', generation=m.generation+1, \
               lease_until=now()+interval '120 seconds' FROM picked WHERE m.id=picked.id \
             RETURNING m.id,m.generation,m.purpose,m.event_id,m.recipient_user_id",
        )
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
    pub async fn finish(&self, claim: MailClaim, outcome: MailOutcome) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE mail_outbox SET status=$3, lease_until=NULL, finished_at=now() \
             WHERE id=$1 AND generation=$2 AND status='leased' AND lease_until>now()",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(outcome.as_str())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// A dead worker might have sent its message before losing its connection.
    /// Quarantine expired claims, rather than silently risking duplicate mail.
    pub async fn recover_expired(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "WITH expired AS (SELECT id FROM mail_outbox WHERE status='leased' \
               AND lease_until<=now() ORDER BY lease_until,id LIMIT 100 FOR UPDATE SKIP LOCKED) \
             UPDATE mail_outbox m SET status='uncertain', lease_until=NULL, finished_at=now() \
             FROM expired WHERE m.id=expired.id",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}
