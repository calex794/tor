//! A-03: bounded, owner-only active sessions. Never expose token hashes.
use super::PgAccounts;
use crate::services::security::{SESSION_AGE_SECONDS, SESSION_RENEWAL_INTERVAL_SECONDS};
use std::time::Duration;

pub struct ActiveSession {
    pub id: String,
    pub opened_utc: String,
    pub expires_utc: String,
    pub browser_family: String,
    pub current: bool,
}

impl PgAccounts {
    /// Extend only a still-live session following accepted browser activity.
    /// Return the remaining server lifetime even when no write was necessary:
    /// a browser that missed a previous Set-Cookie can synchronize again.
    pub async fn renew_session_on_activity(
        &self,
        digest: &[u8; 32],
    ) -> Result<Option<u32>, sqlx::Error> {
        tokio::time::timeout(Duration::from_millis(300), self.renew_session_inner(digest))
            .await
            .map_err(|_| sqlx::Error::PoolTimedOut)?
    }

    async fn renew_session_inner(&self, digest: &[u8; 32]) -> Result<Option<u32>, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("SELECT set_config('statement_timeout','200ms',true),set_config('lock_timeout','25ms',true)")
            .execute(&mut *tx).await?;
        // No row is changed until at least one day has elapsed since the last
        // full TTL. A live legacy seven-day session is eligible immediately.
        sqlx::query(
            "UPDATE sessions s
             SET expires_at=now()+($2::double precision * INTERVAL '1 second')
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
               AND s.expires_at<=now()+($3::double precision * INTERVAL '1 second')
               AND EXISTS(SELECT 1 FROM users u WHERE u.id=s.user_id AND u.account_state='active')",
        )
        .bind(digest.as_slice())
        .bind(f64::from(SESSION_AGE_SECONDS))
        .bind(f64::from(
            SESSION_AGE_SECONDS - SESSION_RENEWAL_INTERVAL_SECONDS,
        ))
        .execute(&mut *tx)
        .await?;
        // A separate read-committed statement sees a concurrent renewal rather
        // than returning the old expiry from the UPDATE statement's snapshot.
        let seconds: Option<i64> = sqlx::query_scalar(
            "SELECT floor(extract(epoch FROM (s.expires_at-now())))::bigint
             FROM sessions s JOIN users u ON u.id=s.user_id
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
               AND u.account_state='active'",
        )
        .bind(digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(seconds
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .map(|value| value.min(SESSION_AGE_SECONDS)))
    }

    /// The caller is the authenticated owner, not an ID supplied by the browser.
    /// This is a snapshot of at most 50 live sessions, not a "last seen" feed.
    pub async fn list_active_sessions(
        &self,
        user_id: i64,
        current_digest: &[u8; 32],
    ) -> Result<Vec<ActiveSession>, sqlx::Error> {
        let rows: Vec<(String, String, String, String, bool)> = sqlx::query_as(
            r#"SELECT s.session_id::text,
                to_char(s.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI') AS opened_utc,
                to_char(s.expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI') AS expires_utc,
                COALESCE(e.agent_family, 'unknown') AS agent_family,
                (s.token_hash = $2) AS is_current
               FROM sessions s
               LEFT JOIN account_login_events e
                 ON e.session_id = s.session_id AND e.user_id = s.user_id
               WHERE s.user_id = $1 AND s.revoked_at IS NULL AND s.expires_at > now()
               ORDER BY is_current DESC, s.created_at DESC, s.session_id DESC
               LIMIT 50"#,
        )
        .bind(user_id)
        .bind(current_digest.as_slice())
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, opened_utc, expires_utc, browser_family, current)| ActiveSession {
                    id,
                    opened_utc,
                    expires_utc,
                    browser_family,
                    current,
                },
            )
            .collect())
    }

    /// Atomically revoke every other live session of this owner, including
    /// sessions outside the bounded UI list. Lock the account before the
    /// requester, matching password rotation and verified login lock order.
    pub async fn revoke_other_owned_sessions(
        &self,
        user_id: i64,
        current_digest: &[u8; 32],
        expected_password_hash: &str,
    ) -> Result<Option<u64>, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let current_hash: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if current_hash.as_deref() != Some(expected_password_hash) {
            return Ok(None);
        }
        let live: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE user_id=$1 AND token_hash=$2
               AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR UPDATE",
        )
        .bind(user_id)
        .bind(current_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if live.is_none() {
            return Ok(None);
        }
        let count: Option<i64> = sqlx::query_scalar(
            "WITH ended AS (
                UPDATE sessions SET revoked_at=now()
                WHERE user_id=$1 AND token_hash<>$2 AND revoked_at IS NULL
                  AND expires_at>clock_timestamp() RETURNING session_id
             )
             INSERT INTO account_security_events(user_id,event_kind,revoked_count)
             SELECT $1,'owner_revoke_others',count(*) FROM ended HAVING count(*)>0
             RETURNING revoked_count",
        )
        .bind(user_id)
        .bind(current_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let count = u64::try_from(count.unwrap_or(0))
            .map_err(|_| sqlx::Error::Protocol("invalid revocation count".into()))?;
        tx.commit().await?;
        Ok(Some(count))
    }

    /// Revoke a selected owned session only if the requesting session remains live.
    /// Returns Some(true) if the current session revoked itself. Foreign/missing
    /// session IDs return None without revealing another user's existence.
    pub async fn revoke_owned_session(
        &self,
        user_id: i64,
        current_digest: &[u8; 32],
        selected_id: &str,
        expected_password_hash: &str,
    ) -> Result<Option<bool>, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        // Same lock order as password rotation. A credential changed between
        // verification and revocation invalidates the request.
        let current_hash: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if current_hash.as_deref() != Some(expected_password_hash) {
            return Ok(None);
        }
        // Serialize against logout of the requesting session as well.
        let requester: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE user_id=$1 AND token_hash=$2
               AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR UPDATE",
        )
        .bind(user_id)
        .bind(current_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if requester.is_none() {
            return Ok(None);
        }
        let revoked = sqlx::query_scalar(
            r#"WITH ended AS (
                UPDATE sessions AS target SET revoked_at = now()
                WHERE target.user_id = $1 AND target.session_id::text = $2
                  AND target.revoked_at IS NULL AND target.expires_at > clock_timestamp()
                RETURNING target.session_id,(target.token_hash = $3) AS is_current
               ), logged AS (
                INSERT INTO account_security_events(user_id,event_kind,session_id,revoked_count)
                SELECT $1,'owner_revoke',session_id,1 FROM ended RETURNING session_id
               )
               SELECT ended.is_current FROM ended JOIN logged USING(session_id)"#,
        )
        .bind(user_id)
        .bind(selected_id)
        .bind(current_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(revoked)
    }
}
