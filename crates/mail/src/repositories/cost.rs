//! Monthly cost admission in the existing reservation transaction.
use crate::domain::{
    cost::{CostLimits, CostSnapshot, MonthUsage},
    quota::QuotaDecision,
    MailClaim,
};
use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction};

#[derive(Clone)]
pub struct PgMailCost {
    pool: PgPool,
}

impl PgMailCost {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Local estimate, not actual Yandex billing. Never initialize missing
    /// months during a read; missing state is an operator action.
    pub async fn snapshot(&self) -> Result<Option<CostSnapshot>, sqlx::Error> {
        let mut connection = self.pool.acquire().await?;
        load(&mut connection).await
    }
}

async fn load(conn: &mut PgConnection) -> Result<Option<CostSnapshot>, sqlx::Error> {
    let row = sqlx::query(CURRENT_SQL).fetch_optional(conn).await?;
    row.map(|r| {
        let usage = if let Some(month_start) = r.try_get::<Option<String>, _>("month_start")? {
            Some(MonthUsage {
                month_start,
                free_allowance: r.try_get("free_allowance")?,
                baseline_kopecks: r.try_get("baseline_kopecks")?,
                rate_per_thousand: r.try_get("rate_per_thousand")?,
                reserved_attempts: r.try_get("reserved_attempts")?,
            })
        } else {
            None
        };
        Ok(CostSnapshot {
            limits: CostLimits {
                enabled: r.try_get("enabled")?,
                warning_1: r.try_get("warning_1")?,
                warning_2: r.try_get("warning_2")?,
                news_pause: r.try_get("news_pause")?,
                monthly_limit: r.try_get("monthly_limit")?,
            },
            usage,
            boundary_pause: r.try_get("boundary_pause")?,
        })
    })
    .transpose()
}

/// Caller holds the quota singleton lock and has just inserted the
/// reservation. Future configuration writers MUST take that same lock.
/// The monthly increment and SMTP permit commit together or neither does.
pub(super) async fn record_reservation(
    tx: &mut Transaction<'_, Postgres>,
    claim: MailClaim,
) -> Result<QuotaDecision, sqlx::Error> {
    use QuotaDecision::*;
    let Some(snapshot) = load(&mut **tx).await? else {
        return Ok(CostStateUnavailable);
    };
    let decision = snapshot.admission(claim.intent.purpose);
    if decision != Reserved {
        return Ok(decision);
    }
    let Some(usage) = snapshot.usage else {
        return Ok(CostStateUnavailable);
    };
    // Recheck that the inserted attempt and admission belong to the same
    // billing month. A clock/month change must never reset the budget.
    let count = sqlx::query(
        "UPDATE mail_cost_months m SET reserved_attempts=reserved_attempts+1 \
         FROM mail_send_reservations r,mail_cost_policy p \
         WHERE p.singleton AND p.enabled AND m.month_start=$1::date \
           AND m.reserved_attempts=$2 AND r.outbox_id=$3 AND r.generation=$4 \
           AND m.month_start=date_trunc('month',r.reserved_at AT TIME ZONE 'Europe/Moscow')::date \
           AND m.month_start=date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date",
    )
    .bind(&usage.month_start)
    .bind(usage.reserved_attempts)
    .bind(claim.id)
    .bind(claim.generation)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    Ok(if count == 1 {
        Reserved
    } else {
        CostStateUnavailable
    })
}

// Billing detail uses Moscow dates. Defer admission in the final five
// minutes of the month; MAIL-04 must enforce a shorter bounded SMTP attempt
// and prohibit late submission. No assumption that delayed provider billing
// is a hard cap on the actual account.
const CURRENT_SQL: &str = r#"
WITH stamp AS MATERIALIZED (
    SELECT clock_timestamp() AT TIME ZONE 'Europe/Moscow' AS local_at
)
SELECT p.enabled,p.warning_1,p.warning_2,p.news_pause,p.monthly_limit,
       m.month_start::text AS month_start,m.free_allowance,m.baseline_kopecks,
       m.rate_per_thousand,m.reserved_attempts,
       (s.local_at >= date_trunc('month',s.local_at)+interval '1 month'-interval '5 minutes')
         AS boundary_pause
FROM mail_cost_policy p CROSS JOIN stamp s
LEFT JOIN mail_cost_months m ON m.month_start=date_trunc('month',s.local_at)::date
WHERE p.singleton
"#;
