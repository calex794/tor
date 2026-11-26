//! AUTH-05a: account-scoped security timeline, without tokens or private IPs.
use super::PgAccounts;
use std::time::Duration;

#[derive(sqlx::FromRow)]
pub struct SecurityEvent {
    pub event_kind: String,
    pub occurred_utc: String,
    pub browser_family: Option<String>,
    pub revoked_count: Option<i64>,
}

impl PgAccounts {
    /// Return at most 50 genuine recent events for the live session owner.
    /// Authorization and both bounded history reads share one SQL snapshot.
    /// An invalid session reveals no events, regardless of any claimed ID.
    pub async fn security_history(
        &self,
        digest: &[u8; 32],
    ) -> Result<Vec<SecurityEvent>, sqlx::Error> {
        tokio::time::timeout(Duration::from_millis(950), self.read_security_history(digest))
            .await
            .map_err(|_| sqlx::Error::PoolTimedOut)?
    }

    async fn read_security_history(
        &self,
        digest: &[u8; 32],
    ) -> Result<Vec<SecurityEvent>, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "SELECT set_config('statement_timeout','500ms',true),
                    set_config('lock_timeout','25ms',true)",
        )
        .execute(&mut *tx)
        .await?;
        let events = sqlx::query_as(
            "WITH owner AS (
                SELECT s.user_id FROM sessions s JOIN users u ON u.id=s.user_id
                WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
                  AND u.account_state='active'
             ), recent_login AS (
                SELECT e.event_kind,e.created_at,e.agent_family AS browser_family,
                       NULL::bigint AS revoked_count,e.session_id::text AS tie
                FROM owner o JOIN account_login_events e ON e.user_id=o.user_id
                WHERE e.created_at>=now()-INTERVAL '90 days'
                ORDER BY e.created_at DESC,e.session_id DESC LIMIT 50
             ), recent_security AS (
                SELECT e.event_kind,e.created_at,NULL::text AS browser_family,
                       e.revoked_count,e.id::text AS tie
                FROM owner o JOIN account_security_events e ON e.user_id=o.user_id
                WHERE e.created_at>=now()-INTERVAL '90 days'
                ORDER BY e.created_at DESC,e.id DESC LIMIT 50
             )
             SELECT event_kind,
                    to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') AS occurred_utc,
                    browser_family,revoked_count
             FROM (SELECT * FROM recent_login UNION ALL SELECT * FROM recent_security) events
             ORDER BY created_at DESC,event_kind,tie DESC LIMIT 50",
        )
        .bind(digest.as_slice())
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(events)
    }
}
