//! Obtain one coherent observation under the shared quota lock.
//! Snapshot reads never initialize a month or write a receipt; activation is separate.
use crate::domain::{
    cost::{CostLimits, CostSnapshot, MonthUsage},
    launch::{LaunchBinding, LaunchSnapshot},
    postbox_profile::{ProfileIdentity, SealedProfile},
    quota::QuotaDecision,
};
use sqlx::{PgPool, Postgres, Row, Transaction};
use tor_contracts::mail::MailPurpose;

mod activation;
mod dispatch;
pub(super) use dispatch::{dispatch_matches, has_activation_receipt};

#[derive(Clone)]
pub struct PgMailLaunch {
    pool: PgPool,
}

impl PgMailLaunch {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Internal observation only. The HTTP caller must check live authority
    /// and exact password intent. The dispatch caller must re-read after the
    /// committed reservation. None/error is a stop, never a permissive default.
    pub async fn snapshot(&self) -> Result<Option<LaunchSnapshot>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        lock_guard(&mut tx).await?;
        let result = load(&mut tx).await?;
        tx.commit().await?;
        Ok(result)
    }
}

async fn lock_guard(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT singleton FROM mail_send_budget_guard WHERE singleton FOR UPDATE",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(())
}

async fn load(tx: &mut Transaction<'_, Postgres>) -> Result<Option<LaunchSnapshot>, sqlx::Error> {
    let row = sqlx::query(SNAPSHOT_SQL).fetch_optional(&mut **tx).await?;
    row.map(decode).transpose()
}

fn invalid() -> sqlx::Error {
    sqlx::Error::Protocol("invalid mail launch observation".into())
}

fn decode(row: sqlx::postgres::PgRow) -> Result<LaunchSnapshot, sqlx::Error> {
    let profile = SealedProfile {
        identity: ProfileIdentity::new(row.try_get("identity_id")?, row.try_get("sender")?)
            .map_err(|_| invalid())?,
        revision: row.try_get("profile_revision")?,
        key_version: row.try_get("key_version")?,
        nonce: row
            .try_get::<Vec<u8>, _>("nonce")?
            .try_into()
            .map_err(|_| invalid())?,
        ciphertext: row.try_get("ciphertext")?,
    };
    if !profile.valid() {
        return Err(invalid());
    }
    let limits = CostLimits {
        enabled: true,
        warning_1: row.try_get("warning_1")?,
        warning_2: row.try_get("warning_2")?,
        news_pause: row.try_get("news_pause")?,
        monthly_limit: row.try_get("monthly_limit")?,
    };
    let usage = MonthUsage {
        month_start: row.try_get("month_start")?,
        free_allowance: row.try_get("free_allowance")?,
        baseline_kopecks: row.try_get("baseline_kopecks")?,
        rate_per_thousand: row.try_get("rate_per_thousand")?,
        reserved_attempts: row.try_get("reserved_attempts")?,
    };
    let boundary_pause = row.try_get("boundary_pause")?;
    let reconciled_attempts = row.try_get("new_reserved_attempts")?;
    let binding = LaunchBinding {
        postmaster_epoch: row.try_get("postmaster_epoch")?,
        database_oid: row.try_get("database_oid")?,
        profile_digest: profile.intent_digest(),
        month: usage.month_start.clone(),
        evidence: row
            .try_get::<Vec<u8>, _>("evidence_sha256")?
            .try_into()
            .map_err(|_| invalid())?,
        reconciliation_revision: row.try_get("reconciliation_revision")?,
        limits: [
            limits.warning_1,
            limits.warning_2,
            limits.news_pause,
            limits.monthly_limit,
        ],
        pricing: [
            usage.free_allowance,
            usage.baseline_kopecks,
            usage.rate_per_thousand,
        ],
        reconciled_attempts,
    };
    let reserved_attempts = usage.reserved_attempts;
    let receipt_profile: Option<Vec<u8>> = row.try_get("activation_profile_digest")?;
    let activation_digest = if receipt_profile.as_deref() == Some(binding.profile_digest.as_slice())
    {
        row.try_get::<Option<Vec<u8>>, _>("activation_digest")?
            .map(|value| value.try_into().map_err(|_| invalid()))
            .transpose()?
    } else {
        None
    };
    let cost = CostSnapshot {
        limits,
        usage: Some(usage),
        boundary_pause,
    };
    // These are general budget checks only. The real per-purpose quota
    // and news reserve are still mandatory at reservation and submission.
    Ok(LaunchSnapshot {
        binding,
        policy_revision: row.try_get("policy_revision")?,
        activation_digest,
        enabled: row.try_get("enabled")?,
        boundary_pause,
        reserved_attempts,
        room_for_reservation: cost.admission(MailPurpose::SupportReply) == QuotaDecision::Reserved,
        within_budget: cost.submission(MailPurpose::SupportReply) == QuotaDecision::Reserved,
    })
}

const SNAPSHOT_SQL: &str = r#"
WITH stamp AS MATERIALIZED (
    SELECT clock_timestamp() AT TIME ZONE 'Europe/Moscow' AS local_at,
           clock_timestamp() AS checked_at
)
SELECT (extract(epoch FROM pg_postmaster_start_time())*1000000)::bigint AS postmaster_epoch,
       (SELECT oid::bigint FROM pg_database WHERE datname=current_database()) AS database_oid,
       p.revision AS policy_revision,p.enabled,p.warning_1,p.warning_2,p.news_pause,p.monthly_limit,
       m.month_start::text AS month_start,m.free_allowance,m.baseline_kopecks,
       m.rate_per_thousand,m.reserved_attempts,
       (s.local_at>=date_trunc('month',s.local_at)+interval '1 month'-interval '5 minutes')
         AS boundary_pause,
       a.new_revision AS reconciliation_revision,a.evidence_sha256,a.new_reserved_attempts,
       f.revision AS profile_revision,f.identity_id,f.sender,f.key_version,f.nonce,f.ciphertext,
       CASE WHEN p.enabled THEN l.intent_digest END AS activation_digest,
       CASE WHEN p.enabled THEN l.profile_digest END AS activation_profile_digest
FROM stamp s
JOIN mail_cost_months m ON m.month_start=date_trunc('month',s.local_at)::date
CROSS JOIN mail_cost_policy p
CROSS JOIN mail_postbox_profile f
JOIN LATERAL (
    SELECT new_revision,evidence_sha256,new_free_allowance,new_baseline_kopecks,
           new_rate_per_thousand,new_reserved_attempts,created_at
    FROM mail_cost_month_reconciliation_audit
    WHERE month_start=m.month_start
    ORDER BY new_revision DESC LIMIT 1
) a ON true
LEFT JOIN mail_launch_activation_audit l
  ON l.new_revision=p.revision AND l.old_revision=a.new_revision
 AND l.month_start=m.month_start AND l.evidence_sha256=a.evidence_sha256
 AND l.postmaster_epoch=(extract(epoch FROM pg_postmaster_start_time())*1000000)::bigint
 AND l.database_oid=(SELECT oid::bigint FROM pg_database WHERE datname=current_database())
 AND l.reserved_attempts=a.new_reserved_attempts AND l.created_at<=s.checked_at
WHERE p.singleton AND f.singleton AND f.ciphertext IS NOT NULL
  AND a.new_revision<=p.revision AND a.created_at<=s.checked_at
  AND a.new_free_allowance=m.free_allowance
  AND a.new_baseline_kopecks=m.baseline_kopecks
  AND a.new_rate_per_thousand=m.rate_per_thousand
  AND a.new_reserved_attempts<=m.reserved_attempts
"#;
