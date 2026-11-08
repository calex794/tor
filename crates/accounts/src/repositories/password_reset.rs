//! One-time reset proof, credential-bound and serialized with password login/change.
use super::PgAccounts;
use crate::services::security::session_token_digest;

impl PgAccounts {
    /// Cheap proof lookup before scheduling any expensive password job.
    /// Consumption still rechecks under locks after hashing.
    pub async fn live_reset_password_hash(
        &self,
        digest: &[u8; 32],
    ) -> Result<Option<String>, sqlx::Error> {
        self.live_reset_password_hash_allowed(digest, true, true)
            .await
    }

    /// Remain compatible with legacy email-reset schema while A/S004 is off.
    /// JSON key lookup treats missing pre-migration `source` as email only.
    pub async fn live_reset_password_hash_allowed(
        &self,
        digest: &[u8; 32],
        email_enabled: bool,
        telegram_enabled: bool,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT u.password_hash,t.credential_fingerprint FROM password_reset_tokens t
             JOIN users u ON u.id=t.user_id WHERE t.token_hash=$1
             AND t.consumed_at IS NULL AND t.expires_at>now() AND u.account_state='active'
             AND ((COALESCE(to_jsonb(t)->>'source','email')='email' AND $2)
               OR (COALESCE(to_jsonb(t)->>'source','email') IN ('telegram','backup') AND $3))
             AND (COALESCE(to_jsonb(t)->>'source','email')='email' OR (
               NOT EXISTS (SELECT 1 FROM account_roles r
                           WHERE r.user_id=u.id AND r.is_active)
               AND NOT EXISTS (SELECT 1 FROM books b WHERE b.owner_user_id=u.id)
               AND NOT EXISTS (SELECT 1 FROM book_members bm
                               WHERE bm.user_id=u.id AND bm.is_active)
             ))",
        )
        .bind(digest.as_slice())
        .bind(email_enabled)
        .bind(telegram_enabled)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.and_then(|(hash, fingerprint)| {
            (fingerprint.as_slice() == session_token_digest(&hash).as_slice()).then_some(hash)
        }))
    }

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
        self.consume_password_reset_allowed(token_hash, new_hash, true, true)
            .await
    }

    pub async fn consume_password_reset_allowed(
        &self,
        token_hash: &[u8; 32],
        new_hash: &str,
        email_enabled: bool,
        telegram_enabled: bool,
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
             WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now()
             AND ((COALESCE(to_jsonb(password_reset_tokens)->>'source','email')='email' AND $2)
               OR (COALESCE(to_jsonb(password_reset_tokens)->>'source','email')
                   IN ('telegram','backup') AND $3))
             AND (COALESCE(to_jsonb(password_reset_tokens)->>'source','email')='email' OR (
               NOT EXISTS (SELECT 1 FROM account_roles r
                           WHERE r.user_id=password_reset_tokens.user_id AND r.is_active)
               AND NOT EXISTS (SELECT 1 FROM books b
                               WHERE b.owner_user_id=password_reset_tokens.user_id)
               AND NOT EXISTS (SELECT 1 FROM book_members bm
                               WHERE bm.user_id=password_reset_tokens.user_id AND bm.is_active)
             ))
             FOR UPDATE",
        )
        .bind(token_hash.as_slice())
        .bind(email_enabled)
        .bind(telegram_enabled)
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
