//! Credential writes lock the account before sessions to serialize login and rotation.
use super::postgres::{PgAccounts, INSERT_AUDITED_SESSION};
use crate::domain::access_audit::LoginAuditContext;
use std::num::NonZeroU32;

impl PgAccounts {
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
        user_id: i64,
        old_session: &[u8; 32],
        expected_password_hash: &str,
        new_password_hash: &str,
        new_session: &[u8; 32],
        ttl: NonZeroU32,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id = $1 FOR UPDATE",
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
        sqlx::query(
            "INSERT INTO sessions (user_id, token_hash, expires_at)
             VALUES ($1, $2, now() + ($3::double precision * INTERVAL '1 second'))",
        )
        .bind(user_id)
        .bind(new_session.as_slice())
        .bind(ttl.get() as f64)
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
            "SELECT password_hash FROM users WHERE id = $1 FOR UPDATE",
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
