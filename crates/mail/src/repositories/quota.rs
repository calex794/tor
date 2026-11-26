//! PostgreSQL serialization point for all outbound quota reservations.
use crate::domain::{
    quota::{
        QuotaDecision, QuotaKeys, DAILY_LIMIT, REGISTRATION_IP_DAILY, REGISTRATION_RECIPIENT_DAILY,
        TRANSACTIONAL_RESERVE,
    },
    MailClaim,
};
use sqlx::PgPool;
use tor_contracts::mail::MailPurpose;

#[derive(Clone)]
pub struct PgMailQuota {
    pool: PgPool,
}

impl PgMailQuota {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Reserve immediately before a single provider submission. Never hold
    /// this transaction while doing DNS/SMTP. On errors or ambiguous commit,
    /// do not send. This is a budget permit, not a delivery or SMTP receipt.
    pub async fn reserve(
        &self,
        claim: MailClaim,
        keys: QuotaKeys,
    ) -> Result<QuotaDecision, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Serialize all providers/credentials/workers against the same initial
        // budget. Do not use transaction-start time after waiting for this lock.
        sqlx::query_scalar::<_, bool>(
            "SELECT singleton FROM mail_send_budget_guard WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;

        let active = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM mail_outbox WHERE id=$1 AND generation=$2 \
             AND purpose=$3 AND event_id=$4 AND recipient_user_id=$5 \
             AND status='leased' AND lease_until>clock_timestamp() FOR UPDATE",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(claim.intent.purpose.as_str())
        .bind(claim.intent.event_id.get())
        .bind(claim.intent.recipient.get())
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            tx.rollback().await?;
            return Ok(QuotaDecision::StaleClaim);
        }

        let duplicate = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM mail_send_reservations \
             WHERE outbox_id=$1 AND generation=$2)",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .fetch_one(&mut *tx)
        .await?;
        if duplicate {
            tx.rollback().await?;
            return Ok(QuotaDecision::AlreadyReserved);
        }

        // Keep at least two full rolling windows, independent of user deletion.
        sqlx::query(
            "DELETE FROM mail_send_reservations \
             WHERE reserved_at<=clock_timestamp()-interval '48 hours'",
        )
        .execute(&mut *tx)
        .await?;

        let ip = if claim.intent.purpose == MailPurpose::RegistrationConfirmation {
            keys.registration_ip.as_ref().map(|key| key.as_slice())
        } else {
            None
        };
        let decision: String = sqlx::query_scalar(RESERVE_SQL)
            .bind(claim.id)
            .bind(claim.generation)
            .bind(claim.intent.purpose.as_str())
            .bind(keys.recipient.as_slice())
            .bind(ip)
            .bind(DAILY_LIMIT)
            .bind(DAILY_LIMIT - TRANSACTIONAL_RESERVE)
            .bind(REGISTRATION_RECIPIENT_DAILY)
            .bind(REGISTRATION_IP_DAILY)
            .fetch_one(&mut *tx)
            .await?;
        let decision = QuotaDecision::parse(&decision)
            .ok_or_else(|| sqlx::Error::Protocol("invalid mail quota decision".to_owned()))?;
        tx.commit().await?;
        Ok(decision)
    }
}

// A single database timestamp defines the rolling windows and insertion.
// Every predicate uses strict lower bounds: at exactly 24h the old attempt
// has left the window. Future-dated rows count conservatively.
const RESERVE_SQL: &str = r#"
WITH stamp AS MATERIALIZED (
    SELECT clock_timestamp() AS at
), counts AS (
    SELECT count(*) AS total,
           coalesce(bool_or(r.reserved_at > s.at - interval '1 second'), false) AS paced,
           count(*) FILTER (
             WHERE r.purpose='registration_confirmation' AND r.recipient_key=$4
           ) AS recipient_total,
           coalesce(bool_or(
             r.purpose='registration_confirmation' AND r.recipient_key=$4
             AND r.reserved_at > s.at - interval '120 seconds'
           ), false) AS recipient_recent,
           count(*) FILTER (
             WHERE r.purpose='registration_confirmation'
               AND r.registration_ip_key IS NOT DISTINCT FROM $5
           ) AS ip_total
    FROM stamp s JOIN mail_send_reservations r
      ON r.reserved_at > s.at - interval '24 hours'
), decision AS (
    SELECT CASE
      WHEN NOT EXISTS (
        SELECT 1 FROM mail_outbox m, stamp s
        WHERE m.id=$1 AND m.generation=$2 AND m.status='leased' AND m.lease_until>s.at
      ) THEN 'stale_claim'
      WHEN total >= $6 THEN 'global_daily'
      WHEN $3='project_news' AND total >= $7 THEN 'transactional_reserve'
      WHEN $3='registration_confirmation' AND recipient_total >= $8 THEN 'recipient_daily'
      WHEN $3='registration_confirmation' AND recipient_recent THEN 'recipient_cooldown'
      WHEN $3='registration_confirmation' AND ip_total >= $9 THEN 'ip_daily'
      WHEN paced THEN 'global_pace'
      ELSE 'reserved'
    END AS outcome FROM counts
), inserted AS (
    INSERT INTO mail_send_reservations (
        outbox_id, generation, purpose, recipient_key, registration_ip_key, reserved_at
    )
    SELECT $1,$2,$3,$4,$5,s.at FROM decision d, stamp s WHERE d.outcome='reserved'
    RETURNING outbox_id
)
SELECT CASE WHEN d.outcome='reserved' AND NOT EXISTS (SELECT 1 FROM inserted)
            THEN 'stale_claim' ELSE d.outcome END
FROM decision d
"#;
