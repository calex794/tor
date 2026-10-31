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
        sqlx::query(
            "WITH batch AS (
                SELECT scope_hash,bucket_start FROM book_engagement_rate_buckets
                WHERE bucket_start<now()-INTERVAL '2 hours'
                ORDER BY bucket_start,scope_hash LIMIT 5000 FOR UPDATE SKIP LOCKED
             )
             DELETE FROM book_engagement_rate_buckets b USING batch
             WHERE b.scope_hash=batch.scope_hash AND b.bucket_start=batch.bucket_start",
        )
        .execute(self.pool())
        .await?;
        sqlx::query(
            "WITH batch AS (
                SELECT id FROM book_engagement_integrity_events
                WHERE observed_at<now()-INTERVAL '30 days'
                ORDER BY observed_at,id LIMIT 1000 FOR UPDATE SKIP LOCKED
             )
             DELETE FROM book_engagement_integrity_events e USING batch
             WHERE e.id=batch.id",
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
