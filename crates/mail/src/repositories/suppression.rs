//! Durable recipient suppression, sharing the quota serialization point.
use crate::domain::suppression::{AttemptRef, DeliveryFeedback, FeedbackDecision};
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgMailSuppression {
    pool: PgPool,
}

impl PgMailSuppression {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Internal trusted input only. This does not authenticate provider events.
    /// Resolve the actual recipient from the ledger, not from callback text.
    /// Correlation with external message IDs and late events is MAIL-01g2.
    pub async fn record_verified_feedback(
        &self,
        attempt: AttemptRef,
        feedback: DeliveryFeedback,
    ) -> Result<FeedbackDecision, sqlx::Error> {
        let Some(reason) = feedback.suppression_reason() else {
            return Ok(FeedbackDecision::Ignored);
        };
        let mut tx = self.pool.begin().await?;
        let binding = sqlx::query_scalar::<_, Option<Vec<u8>>>(
            "SELECT key_binding FROM mail_send_budget_guard WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if binding.is_none() {
            tx.rollback().await?;
            return Ok(FeedbackDecision::UnboundKey);
        }

        let outcome: String = sqlx::query_scalar(RECORD_SQL)
            .bind(attempt.outbox_id)
            .bind(attempt.generation)
            .bind(reason.as_str())
            .fetch_one(&mut *tx)
            .await?;
        let decision = match outcome.as_str() {
            "applied" => FeedbackDecision::Applied,
            "unchanged" => FeedbackDecision::Unchanged,
            "uncorrelated" => FeedbackDecision::Uncorrelated,
            _ => {
                return Err(sqlx::Error::Protocol(
                    "invalid mail feedback outcome".to_owned(),
                ))
            }
        };
        tx.commit().await?;
        Ok(decision)
    }

    /// Expired hard bounces are removable; complaints require a separately
    /// authorized review flow. No network calls or unbounded delete.
    pub async fn prune_expired(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "WITH batch AS (SELECT recipient_key FROM mail_recipient_suppression \
             WHERE suppressed_until<=clock_timestamp() \
             ORDER BY suppressed_until,recipient_key LIMIT 100 FOR UPDATE SKIP LOCKED) \
             DELETE FROM mail_recipient_suppression s USING batch b \
             WHERE s.recipient_key=b.recipient_key",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

// Use the immutable attempt time, not arrival time, so duplicate callbacks
// cannot extend the block. A complaint cannot be downgraded by a bounce.
const RECORD_SQL: &str = r#"
WITH stamp AS MATERIALIZED (SELECT clock_timestamp() AS at),
source AS MATERIALIZED (
    SELECT r.recipient_key,r.reserved_at FROM mail_send_reservations r,stamp s
    WHERE r.outbox_id=$1 AND r.generation=$2
      AND r.reserved_at>s.at-interval '48 hours' AND r.reserved_at<=s.at
), changed AS (
    INSERT INTO mail_recipient_suppression AS existing
        (recipient_key,reason,source_reserved_at,suppressed_until)
    SELECT recipient_key,$3,reserved_at,
           CASE WHEN $3='hard_bounce' THEN reserved_at+interval '30 days' ELSE NULL END
    FROM source
    ON CONFLICT (recipient_key) DO UPDATE SET
        reason=excluded.reason,
        source_reserved_at=excluded.source_reserved_at,
        suppressed_until=excluded.suppressed_until
    WHERE existing.reason='hard_bounce'
      AND (excluded.reason='complaint'
           OR excluded.source_reserved_at>existing.source_reserved_at)
    RETURNING recipient_key
)
SELECT CASE WHEN EXISTS(SELECT 1 FROM changed) THEN 'applied'
            WHEN EXISTS(SELECT 1 FROM source) THEN 'unchanged'
            ELSE 'uncorrelated' END
"#;
