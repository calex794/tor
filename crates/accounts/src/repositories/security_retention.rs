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
        // Expired spray sources cascade-delete their distinct target set.
        // Limit each cleanup batch and skip rows held by live login transactions.
        sqlx::query(
            "WITH batch AS (
               SELECT scope,source_key FROM auth_spray_windows
               WHERE touched_at < clock_timestamp()-INTERVAL '1 day'
               ORDER BY touched_at LIMIT 1000 FOR UPDATE SKIP LOCKED
             )
             DELETE FROM auth_spray_windows w USING batch b
             WHERE w.scope=b.scope AND w.source_key=b.source_key",
        )
        .execute(self.pool())
        .await?;
        // AB-08: old actor windows cascade-delete their admitted command keys.
        // Bounded batch and SKIP LOCKED preserve live writers and short locks.
        sqlx::query(
            "WITH batch AS (
                 SELECT scope,actor_id FROM scoped_action_windows
                 WHERE touched_at < clock_timestamp()-INTERVAL '1 day'
                 ORDER BY touched_at LIMIT 1000 FOR UPDATE SKIP LOCKED
             )
             DELETE FROM scoped_action_windows w USING batch b
             WHERE w.scope=b.scope AND w.actor_id=b.actor_id",
        )
        .execute(self.pool())
        .await?;
        // Telegram uses a separate, explicitly migrated schema. Existing
        // account retention MUST still work while its feature flag is off and
        // this additive migration has not been applied to an existing DB.
        let telegram_schema_present: bool = sqlx::query_scalar(
            "SELECT to_regclass('account_telegram_recovery_requests') IS NOT NULL",
        )
        .fetch_one(self.pool())
        .await?;
        if telegram_schema_present {
            sqlx::query(
                "WITH batch AS (
                    SELECT user_id FROM account_telegram_link_intents
                    WHERE expires_at<now()-INTERVAL '1 hour'
                    ORDER BY expires_at LIMIT 1000 FOR UPDATE SKIP LOCKED
                 )
                 DELETE FROM account_telegram_link_intents i USING batch b WHERE i.user_id=b.user_id",
            )
            .execute(self.pool())
            .await?;
            sqlx::query(
                "WITH batch AS (
                    SELECT update_id FROM account_telegram_bot_updates
                    WHERE received_at<now()-INTERVAL '2 days'
                    ORDER BY received_at LIMIT 1000 FOR UPDATE SKIP LOCKED
                 )
                 DELETE FROM account_telegram_bot_updates u USING batch b WHERE u.update_id=b.update_id",
            )
            .execute(self.pool())
            .await?;
            sqlx::query(
                "WITH batch AS (
                   SELECT id FROM account_telegram_binding_events
                   WHERE created_at<now()-INTERVAL '90 days'
                   ORDER BY created_at LIMIT 1000 FOR UPDATE SKIP LOCKED
                 )
                 DELETE FROM account_telegram_binding_events e USING batch b WHERE e.id=b.id",
            )
            .execute(self.pool())
            .await?;
            sqlx::query(
                "WITH batch AS (
                   SELECT id FROM account_telegram_recovery_requests
                   WHERE expires_at<now()-INTERVAL '1 hour'
                   ORDER BY expires_at LIMIT 1000 FOR UPDATE SKIP LOCKED
                 )
                 DELETE FROM account_telegram_recovery_requests t
                 USING batch b WHERE t.id=b.id",
            )
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }
}
