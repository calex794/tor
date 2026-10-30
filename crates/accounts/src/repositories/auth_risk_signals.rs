//! A-09: bounded, read-only operator signals; never authorize or block logins.
//! Thresholds are minimum-sample safeguards, not calibrated production baselines.
use crate::repositories::PgAccounts;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuthRiskSignal {
    pub category: String,
    pub key_hex: Option<String>,
    pub attempts: i64,
    pub distinct_targets: i64,
    pub distinct_sources: i64,
    pub bad_passwords: i64,
    pub unknown_accounts: i64,
    pub successes: i64,
}

impl PgAccounts {
    /// Limited output and seven-day source data. Null source IPs cannot become
    /// a misleading shared "anonymous IP" incident.
    pub async fn auth_risk_signals(&self) -> Result<Vec<AuthRiskSignal>, sqlx::Error> {
        sqlx::query_as(
            "WITH recent AS (
               SELECT outcome, target_key, source_key
               FROM auth_login_observations
               WHERE observed_at >= now()-INTERVAL '15 minutes'
             ), by_source AS (
               SELECT 'source_spray'::text AS category,
                      encode(source_key,'hex') AS key_hex,
                      count(*) AS attempts,
                      count(DISTINCT target_key) AS distinct_targets,
                      count(DISTINCT source_key) AS distinct_sources,
                      count(*) FILTER (WHERE outcome='bad_password') AS bad_passwords,
                      count(*) FILTER (WHERE outcome='unknown_account') AS unknown_accounts,
                      count(*) FILTER (WHERE outcome='success') AS successes
               FROM recent WHERE source_key IS NOT NULL AND target_key IS NOT NULL
               GROUP BY source_key
               HAVING count(*) >= 15 AND count(DISTINCT target_key) >= 10
             ), by_target AS (
               SELECT 'distributed_target'::text AS category,
                      encode(target_key,'hex') AS key_hex,
                      count(*) AS attempts,
                      count(DISTINCT target_key) AS distinct_targets,
                      count(DISTINCT source_key) AS distinct_sources,
                      count(*) FILTER (WHERE outcome='bad_password') AS bad_passwords,
                      count(*) FILTER (WHERE outcome='unknown_account') AS unknown_accounts,
                      count(*) FILTER (WHERE outcome='success') AS successes
               FROM recent WHERE target_key IS NOT NULL AND source_key IS NOT NULL
               GROUP BY target_key
               HAVING count(*) >= 15 AND count(DISTINCT source_key) >= 10
             )
             SELECT * FROM (
               SELECT * FROM by_source WHERE bad_passwords+unknown_accounts >= 10
               UNION ALL
               SELECT * FROM by_target WHERE bad_passwords+unknown_accounts >= 10
             ) signals
             ORDER BY attempts DESC, category, key_hex LIMIT 50",
        )
        .fetch_all(self.pool())
        .await
    }
}
