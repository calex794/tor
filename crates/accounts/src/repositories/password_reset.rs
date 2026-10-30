//! One-time reset proof, credential-bound and serialized with password login/change.
use super::PgAccounts;
use crate::services::security::session_token_digest;

impl PgAccounts {
    pub async fn issue_password_reset(
        &self,
        user_id: i64,
        expected_hash: &str,
        token_hash: &[u8; 32],
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let hash: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if hash.as_deref() != Some(expected_hash) {
            return Ok(false);
        }
        sqlx::query("UPDATE password_reset_tokens SET consumed_at=now() WHERE user_id=$1 AND consumed_at IS NULL")
            .bind(user_id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO password_reset_tokens(token_hash,user_id,credential_fingerprint) VALUES($1,$2,$3)")
            .bind(token_hash.as_slice()).bind(user_id).bind(session_token_digest(expected_hash).as_slice())
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Reset creates no authenticated session: the reader must log in afterwards.
    /// Lock order is account -> reset proof -> sessions, as in credential rotation.
    pub async fn consume_password_reset(
        &self,
        token_hash: &[u8; 32],
        new_hash: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let user_id: Option<i64> =
            sqlx::query_scalar("SELECT user_id FROM password_reset_tokens WHERE token_hash=$1")
                .bind(token_hash.as_slice())
                .fetch_optional(&mut *tx)
                .await?;
        let Some(user_id) = user_id else {
            return Ok(false);
        };
        let current: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(current) = current else {
            return Ok(false);
        };
        let proof: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT credential_fingerprint FROM password_reset_tokens
             WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now() FOR UPDATE",
        )
        .bind(token_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if proof.as_deref() != Some(session_token_digest(&current).as_slice()) {
            return Ok(false);
        }
        sqlx::query("UPDATE users SET password_hash=$2 WHERE id=$1")
            .bind(user_id)
            .bind(new_hash)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE sessions SET revoked_at=now() WHERE user_id=$1 AND revoked_at IS NULL")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE password_reset_tokens SET consumed_at=now() WHERE user_id=$1 AND consumed_at IS NULL")
            .bind(user_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}
