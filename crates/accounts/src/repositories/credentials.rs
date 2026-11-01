//! Credential writes lock the account before sessions to serialize login and rotation.
use super::postgres::{PgAccounts, INSERT_AUDITED_SESSION};
use crate::domain::access_audit::LoginAuditContext;
use std::num::NonZeroU32;

/// Private audit data for a proven live session; deliberately not serializable.
#[derive(sqlx::FromRow)]
pub struct SessionLoginAudit {
    pub verified_ip: Option<String>,
    pub event_kind: String,
}

/// One atomic credential change; secret fields deliberately have no Debug output.
pub struct PasswordRotation<'a> {
    pub user_id: i64,
    pub old_session: &'a [u8; 32],
    pub expected_password_hash: &'a str,
    pub new_password_hash: &'a str,
    pub new_session: &'a [u8; 32],
    pub ttl: NonZeroU32,
    pub audit: LoginAuditContext,
}

impl PgAccounts {
    /// Internal audit lookup bound to a live session proof, not a public DTO.
    /// The caller must never serialize this private IP/event pair to the browser.
    pub async fn active_session_login_audit(
        &self,
        digest: &[u8; 32],
    ) -> Result<Option<SessionLoginAudit>, sqlx::Error> {
        sqlx::query_as(
            "SELECT host(e.verified_ip) AS verified_ip, e.event_kind FROM account_login_events e
             JOIN sessions s ON s.session_id=e.session_id
             JOIN users u ON u.id=s.user_id
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL
               AND s.expires_at>now() AND u.account_state='active'",
        )
        .bind(digest.as_slice())
        .fetch_optional(self.pool())
        .await
    }

    /// Read only the hash required for an already authenticated password change.
    pub async fn password_hash_for_user(
        &self,
        user_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(self.pool())
            .await
    }

    /// Compare-and-swap credentials while holding the account lock.
    /// Revoke every previous session and establish one fresh session atomically.
    /// A concurrent password update or logout fails closed.
    pub async fn rotate_password_and_sessions(
        &self,
        command: PasswordRotation<'_>,
    ) -> Result<bool, sqlx::Error> {
        let PasswordRotation {
            user_id,
            old_session,
            expected_password_hash,
            new_password_hash,
            new_session,
            ttl,
            audit,
        } = command;
        let mut tx = self.pool().begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id = $1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if current.as_deref() != Some(expected_password_hash) {
            return Ok(false);
        }
        let live: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions
             WHERE user_id = $1 AND token_hash = $2
               AND revoked_at IS NULL AND expires_at > now() FOR UPDATE",
        )
        .bind(user_id)
        .bind(old_session.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if live.is_none() {
            return Ok(false);
        }

        let updated = sqlx::query(
            "UPDATE users SET password_hash = $1
             WHERE id = $2 AND password_hash = $3",
        )
        .bind(new_password_hash)
        .bind(user_id)
        .bind(expected_password_hash)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE sessions SET revoked_at = now()
             WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(INSERT_AUDITED_SESSION)
            .bind(user_id)
            .bind(new_session.as_slice())
            .bind(ttl.get() as f64)
            .bind(audit.verified_ip.map(|ip| ip.to_string()))
            .bind(audit.agent_family.label())
            .bind(audit.event_kind.label())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Recheck the hash verified by Argon2 under the same lock used by rotation.
    /// A password verified before a rotation cannot mint a session afterwards.
    pub async fn insert_verified_session(
        &self,
        user_id: i64,
        expected_password_hash: &str,
        digest: &[u8; 32],
        ttl: NonZeroU32,
        audit: LoginAuditContext,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id = $1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if current.as_deref() != Some(expected_password_hash) {
            return Ok(false);
        }
        sqlx::query(INSERT_AUDITED_SESSION)
            .bind(user_id)
            .bind(digest.as_slice())
            .bind(ttl.get() as f64)
            .bind(audit.verified_ip.map(|ip| ip.to_string()))
            .bind(audit.agent_family.label())
            .bind(audit.event_kind.label())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}
