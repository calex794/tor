//! A-03: bounded, owner-only active sessions. Never expose token hashes.
use super::PgAccounts;

pub struct ActiveSession {
    pub id: String,
    pub opened_utc: String,
    pub browser_family: String,
    pub current: bool,
}

impl PgAccounts {
    /// The caller is the authenticated owner, not an ID supplied by the browser.
    /// This is a snapshot of at most 50 live sessions, not a "last seen" feed.
    pub async fn list_active_sessions(
        &self,
        user_id: i64,
        current_digest: &[u8; 32],
    ) -> Result<Vec<ActiveSession>, sqlx::Error> {
        let rows: Vec<(String, String, String, bool)> = sqlx::query_as(
            r#"SELECT s.session_id::text,
                to_char(s.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI') AS opened_utc,
                COALESCE(e.agent_family, 'unknown') AS agent_family,
                (s.token_hash = $2) AS is_current
               FROM sessions s
               LEFT JOIN account_login_events e
                 ON e.session_id = s.session_id AND e.user_id = s.user_id
               WHERE s.user_id = $1 AND s.revoked_at IS NULL AND s.expires_at > now()
               ORDER BY s.created_at DESC, s.session_id DESC
               LIMIT 50"#,
        )
        .bind(user_id)
        .bind(current_digest.as_slice())
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, opened_utc, browser_family, current)| ActiveSession {
                id,
                opened_utc,
                browser_family,
                current,
            })
            .collect())
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
        let revoked = sqlx::query_scalar(
            r#"UPDATE sessions AS target SET revoked_at = now()
               WHERE target.user_id = $1 AND target.session_id::text = $2
                 AND target.revoked_at IS NULL AND target.expires_at > now()
                 AND EXISTS (
                   SELECT 1 FROM sessions requester
                   WHERE requester.user_id = $1 AND requester.token_hash = $3
                     AND requester.revoked_at IS NULL AND requester.expires_at > now()
                 )
               RETURNING (target.token_hash = $3)"#,
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
