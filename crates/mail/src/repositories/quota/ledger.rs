//! Shared quota ledger. Caller holds the singleton guard and owns rollback/commit.
use crate::domain::{
    quota::{
        QuotaDecision, QuotaKeys, DAILY_LIMIT, OPERATOR_TEST_RECIPIENT_DAILY,
        PASSWORD_RESET_IP_DAILY, PASSWORD_RESET_RECIPIENT_DAILY, REGISTRATION_IP_DAILY,
        REGISTRATION_RECIPIENT_DAILY, TRANSACTIONAL_RESERVE,
    },
    suppression::SuppressionReason,
};
use sqlx::{Postgres, Transaction};

/// No caller may commit a charged row without its matching monthly cost.
pub(in crate::repositories) async fn reserve_ledger(
    tx: &mut Transaction<'_, Postgres>,
    key_binding: Option<Vec<u8>>,
    id: i64,
    generation: i32,
    purpose: &str,
    keys: QuotaKeys,
    activation_digest: Option<&[u8; 32]>,
) -> Result<QuotaDecision, sqlx::Error> {
    // Never silently replace the key and start fresh recipient/IP buckets.
    // An unbound database with pre-existing reservations also needs an
    // explicit recovery decision; the old digests cannot be translated.
    match key_binding {
        Some(binding) if binding.as_slice() != keys.key_binding() => {
            return Ok(QuotaDecision::KeyMismatch);
        }
        Some(_) => {}
        None => {
            let has_history = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM mail_send_reservations) \
                 OR EXISTS(SELECT 1 FROM mail_recipient_suppression) \
                 OR EXISTS(SELECT 1 FROM mail_feedback_attempts)",
            )
            .fetch_one(&mut **tx)
            .await?;
            if has_history {
                return Ok(QuotaDecision::KeyMismatch);
            }
            sqlx::query("UPDATE mail_send_budget_guard SET key_binding=$1 WHERE singleton")
                .bind(keys.key_binding().as_slice())
                .execute(&mut **tx)
                .await?;
        }
    }

    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM mail_send_reservations \
         WHERE outbox_id=$1 AND generation=$2)",
    )
    .bind(id)
    .bind(generation)
    .fetch_one(&mut **tx)
    .await?;
    if duplicate {
        return Ok(QuotaDecision::AlreadyReserved);
    }

    let suppressed = sqlx::query_scalar::<_, String>(
        "SELECT reason FROM mail_recipient_suppression WHERE recipient_key=$1 \
         AND (suppressed_until IS NULL OR suppressed_until>clock_timestamp())",
    )
    .bind(keys.recipient().as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(reason) = suppressed {
        let reason = SuppressionReason::parse(&reason)
            .ok_or_else(|| sqlx::Error::Protocol("invalid recipient suppression".to_owned()))?;
        return Ok(QuotaDecision::RecipientSuppressed(reason));
    }

    // Keep at least two full rolling windows, independent of user deletion.
    sqlx::query(
        "DELETE FROM mail_send_reservations \
         WHERE reserved_at<=clock_timestamp()-interval '48 hours'",
    )
    .execute(&mut **tx)
    .await?;

    let (recipient_limit, ip_limit) = match purpose {
        "password_reset" => (PASSWORD_RESET_RECIPIENT_DAILY, PASSWORD_RESET_IP_DAILY),
        "operator_test" => (OPERATOR_TEST_RECIPIENT_DAILY, 0),
        _ => (REGISTRATION_RECIPIENT_DAILY, REGISTRATION_IP_DAILY),
    };
    let ip = if matches!(purpose, "registration_confirmation" | "password_reset") {
        keys.registration_ip().map(|key| key.as_slice())
    } else {
        None
    };
    let decision: String = sqlx::query_scalar(RESERVE_SQL)
        .bind(id)
        .bind(generation)
        .bind(purpose)
        .bind(keys.recipient().as_slice())
        .bind(ip)
        .bind(DAILY_LIMIT)
        .bind(DAILY_LIMIT - TRANSACTIONAL_RESERVE)
        .bind(recipient_limit)
        .bind(ip_limit)
        .bind(activation_digest.map(|digest| digest.as_slice()))
        .fetch_one(&mut **tx)
        .await?;
    let decision = QuotaDecision::parse(&decision)
        .ok_or_else(|| sqlx::Error::Protocol("invalid mail quota decision".to_owned()))?;
    Ok(decision)
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
             WHERE r.purpose=$3 AND r.recipient_key=$4
           ) AS recipient_total,
           coalesce(bool_or(
             r.purpose=$3 AND r.recipient_key=$4
             AND r.reserved_at > s.at - interval '120 seconds'
           ), false) AS recipient_recent,
           count(*) FILTER (
             WHERE r.purpose=$3
               AND r.registration_ip_key IS NOT DISTINCT FROM $5
           ) AS ip_total
    FROM stamp s JOIN mail_send_reservations r
      ON r.reserved_at > s.at - interval '24 hours'
), decision AS (
    SELECT CASE
      WHEN NOT EXISTS (
        SELECT 1 FROM mail_outbox m, stamp s
        WHERE m.id=$1 AND m.generation=$2 AND m.purpose=$3
          AND m.status='leased' AND m.lease_until>s.at
      ) THEN 'stale_claim'
      WHEN total >= $6 THEN 'global_daily'
      WHEN $3 IN ('project_news','operator_test') AND total >= $7 THEN 'transactional_reserve'
      WHEN $3='registration_confirmation' AND recipient_total >= $8 THEN 'recipient_daily'
      WHEN $3='registration_confirmation' AND recipient_recent THEN 'recipient_cooldown'
      WHEN $3='registration_confirmation' AND ip_total >= $9 THEN 'ip_daily'
      WHEN $3='password_reset' AND recipient_total >= $8 THEN 'reset_recipient_daily'
      WHEN $3='password_reset' AND recipient_recent THEN 'reset_recipient_cooldown'
      WHEN $3='password_reset' AND ip_total >= $9 THEN 'reset_ip_daily'
      WHEN $3='operator_test' AND recipient_total >= $8 THEN 'operator_test_daily'
      WHEN $3='operator_test' AND recipient_recent THEN 'operator_test_cooldown'
      WHEN paced THEN 'global_pace'
      ELSE 'reserved'
    END AS outcome FROM counts
), inserted AS (
    INSERT INTO mail_send_reservations (
        outbox_id, generation, purpose, recipient_key, registration_ip_key, reserved_at,
        launch_intent_digest
    )
    SELECT $1,$2,$3,$4,$5,s.at,$10 FROM decision d, stamp s WHERE d.outcome='reserved'
    RETURNING outbox_id
)
SELECT CASE WHEN d.outcome='reserved' AND NOT EXISTS (SELECT 1 FROM inserted)
            THEN 'stale_claim' ELSE d.outcome END
FROM decision d
"#;
