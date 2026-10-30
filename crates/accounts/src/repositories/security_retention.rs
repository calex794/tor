//! Bounded A-owned cleanup. No identity labels or reset/session secrets in logs.
use super::PgAccounts;

impl PgAccounts {
    /// Keep IP for at most 30 days and anonymous login-event facts for 90 days.
    /// Run repeatedly in bounded batches; parallel workers skip locked rows.
    pub async fn prune_security_data(&self) -> Result<(), sqlx::Error> {
        sqlx::query("WITH batch AS (SELECT session_id FROM account_login_events
            WHERE created_at<now()-INTERVAL '30 days' AND verified_ip IS NOT NULL
            ORDER BY created_at LIMIT 1000 FOR UPDATE SKIP LOCKED)
            UPDATE account_login_events e SET verified_ip=NULL FROM batch b WHERE e.session_id=b.session_id")
            .execute(self.pool()).await?;
        sqlx::query("WITH batch AS (SELECT session_id FROM account_login_events
            WHERE created_at<now()-INTERVAL '90 days' ORDER BY created_at LIMIT 1000 FOR UPDATE SKIP LOCKED)
            DELETE FROM account_login_events e USING batch b WHERE e.session_id=b.session_id")
            .execute(self.pool()).await?;
        sqlx::query("WITH batch AS (SELECT token_hash FROM password_reset_tokens
            WHERE expires_at<now()-INTERVAL '2 days' ORDER BY expires_at LIMIT 1000 FOR UPDATE SKIP LOCKED)
            DELETE FROM password_reset_tokens t USING batch b WHERE t.token_hash=b.token_hash")
            .execute(self.pool()).await?;
        // Separate failed-login observations have a shorter, bounded lifetime.
        sqlx::query(
            "WITH batch AS (
            SELECT id FROM auth_login_observations
            WHERE observed_at < now()-INTERVAL '7 days'
            ORDER BY observed_at, id LIMIT 1000 FOR UPDATE SKIP LOCKED
        )
        DELETE FROM auth_login_observations o USING batch b WHERE o.id=b.id",
        )
        .execute(self.pool())
        .await?;
        sqlx::query("WITH batch AS (SELECT key_hash FROM auth_attempts
            WHERE period_started<now()-INTERVAL '1 day' ORDER BY period_started LIMIT 1000 FOR UPDATE SKIP LOCKED)
            DELETE FROM auth_attempts t USING batch b WHERE t.key_hash=b.key_hash")
            .execute(self.pool()).await?;
        Ok(())
    }
}
