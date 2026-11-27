//! Obtain one coherent observation under the shared quota lock.
//! Snapshot reads never initialize a month or write a receipt; activation is separate.
use crate::domain::{
    cost::{CostLimits, CostSnapshot, MonthUsage},
    launch::{DispatchGrant, DispatchStart, LaunchBinding, LaunchSnapshot, PreparedDispatch},
    postbox::marker_token,
    postbox_profile::{ProfileIdentity, SealedProfile},
    quota::QuotaDecision,
};
use sqlx::{PgPool, Postgres, Row, Transaction};
use tor_contracts::mail::MailPurpose;

mod activation;

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

    /// Load only the profile covered by the exact process dispatch grant.
    /// No SMTP credentials are decrypted or returned by this repository.
    pub async fn dispatch_profile(
        &self,
        grant: &DispatchGrant,
    ) -> Result<Option<SealedProfile>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        lock_guard(&mut tx).await?;
        if !dispatch_matches(&mut tx, grant, false).await? {
            return Ok(None);
        }
        let row: (String, String, i64, i32, Vec<u8>, Vec<u8>) = sqlx::query_as(
            "SELECT identity_id,sender,revision,key_version,nonce,ciphertext \
             FROM mail_postbox_profile WHERE singleton AND ciphertext IS NOT NULL",
        )
        .fetch_one(&mut *tx)
        .await?;
        let profile = SealedProfile {
            identity: ProfileIdentity::new(row.0, row.1).map_err(|_| invalid())?,
            revision: row.2,
            key_version: row.3,
            nonce: row.4.try_into().map_err(|_| invalid())?,
            ciphertext: row.5,
        };
        if !profile.valid() || &profile.intent_digest() != grant.before.profile_digest() {
            return Ok(None);
        }
        tx.commit().await?;
        Ok(Some(profile))
    }

    /// Last read under the shared guard, after message preparation and
    /// immediately before the single-use authenticated session is consumed.
    /// No lock or transaction may survive into SMTP.
    pub async fn dispatch_start(
        &self,
        prepared: PreparedDispatch,
    ) -> Result<Option<DispatchStart>, sqlx::Error> {
        if !prepared.fresh() {
            return Ok(None);
        }
        let grant = &prepared.grant;
        let claim = grant.claim();
        let Some(digest) = grant.digest() else {
            return Ok(None);
        };
        let Some(marker) = marker_token(prepared.marker()) else {
            return Ok(None);
        };
        let mut tx = self.pool.begin().await?;
        lock_guard(&mut tx).await?;
        if !dispatch_matches(&mut tx, grant, true).await?
            || !super::cost::submission_enabled(&mut tx, claim).await?
        {
            return Ok(None);
        }
        let exact: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(
                SELECT 1 FROM mail_outbox m
                JOIN mail_send_reservations r ON r.outbox_id=m.id AND r.generation=m.generation
                JOIN mail_feedback_attempts f ON f.outbox_id=m.id AND f.generation=m.generation
                JOIN users u ON u.id=m.recipient_user_id
                CROSS JOIN mail_send_budget_guard g
                CROSS JOIN mail_postbox_profile profile
                WHERE m.id=$1 AND m.generation=$2 AND m.purpose=$3
                  AND m.event_id=$4 AND m.recipient_user_id=$5 AND u.account_state='active'
                  AND m.status='leased' AND m.lease_until>clock_timestamp()+interval '21 seconds'
                  AND r.purpose=m.purpose AND r.launch_intent_digest=$6
                  AND f.marker=$7 AND f.recipient_key=r.recipient_key
                  AND g.singleton AND f.key_binding=g.key_binding
                  AND profile.singleton AND f.identity_id=profile.identity_id
                  AND r.reserved_at>clock_timestamp()-interval '120 seconds'
                  AND r.reserved_at<=clock_timestamp()
                  AND date_trunc('month',r.reserved_at AT TIME ZONE 'Europe/Moscow')
                      =date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')
                  AND NOT EXISTS (SELECT 1 FROM mail_recipient_suppression x
                      WHERE x.recipient_key=r.recipient_key
                        AND (x.suppressed_until IS NULL OR x.suppressed_until>clock_timestamp()))
                  AND (m.purpose<>'password_reset' OR EXISTS (
                      SELECT 1 FROM password_reset_tokens p
                      JOIN account_password_reset_mail_requests s
                        ON s.id=p.password_reset_event_id AND s.user_id=p.user_id
                      WHERE p.user_id=u.id AND s.id=m.event_id AND p.token_hash=$8
                        AND p.consumed_at IS NULL AND p.expires_at>clock_timestamp()
                        AND s.created_at>clock_timestamp()-interval '10 minutes'
                        AND s.created_at<=clock_timestamp()
                        AND s.recipient_key=r.recipient_key AND s.key_binding=g.key_binding
                        AND s.request_ip_key IS NOT DISTINCT FROM r.registration_ip_key
                        AND p.credential_fingerprint=s.credential_fingerprint
                        AND p.credential_fingerprint=sha256(convert_to(u.password_hash,'UTF8'))
                  ))
            )"#,
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(claim.intent.purpose.as_str())
        .bind(claim.intent.event_id.get())
        .bind(claim.intent.recipient.get())
        .bind(digest.as_slice())
        .bind(marker)
        .bind(prepared.reset_hash.as_ref().map(|hash| hash.as_slice()))
        .fetch_one(&mut *tx)
        .await?;
        if !exact {
            return Ok(None);
        }
        let Some(current) = load(&mut tx).await? else {
            return Ok(None);
        };
        tx.commit().await?;
        if !prepared.fresh() || !prepared.grant.matches(&current, true) {
            return Ok(None);
        }
        Ok(Some(DispatchStart { prepared, current }))
    }
}

/// Must be called under the shared quota singleton. An enabled cost bit,
/// a recovered receipt, or a test-only grant does not authorize dispatch.
pub(super) async fn dispatch_matches(
    tx: &mut Transaction<'_, Postgres>,
    grant: &DispatchGrant,
    reserved: bool,
) -> Result<bool, sqlx::Error> {
    let Some(current) = load(tx).await? else {
        return Ok(false);
    };
    if !grant.matches(&current, reserved) {
        return Ok(false);
    }
    let Some(digest) = grant.digest() else {
        return Ok(false);
    };
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail_launch_activation_audit \
         WHERE new_revision=$1 AND intent_digest=$2 AND mode='dispatch' AND test_user_id IS NULL)",
    )
    .bind(current.policy_revision())
    .bind(digest.as_slice())
    .fetch_one(&mut **tx)
    .await
}

/// Historical standalone admission methods exist for offline acceptance,
/// not for a currently activated live profile (including operator test).
pub(super) async fn legacy_allowed(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM mail_cost_policy p \
         JOIN mail_launch_activation_audit a ON a.new_revision=p.revision \
         WHERE p.singleton AND p.enabled)",
    )
    .fetch_one(&mut **tx)
    .await
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
