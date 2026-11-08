//! A/S004: optional Telegram identity binding and recovery proofs.
//! Public bot messages never carry TOR identity, password hash or session tokens.
use super::PgAccounts;
use crate::services::security::session_token_digest;

pub struct TelegramLinkState {
    pub linked_user_id: Option<i64>,
    pub candidate_user_id: Option<i64>,
}

impl PgAccounts {
    pub async fn telegram_link_state(
        &self,
        user_id: i64,
        session_digest: &[u8; 32],
    ) -> Result<TelegramLinkState, sqlx::Error> {
        let linked_user_id = sqlx::query_scalar(
            "SELECT telegram_user_id FROM account_telegram_bindings WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        let candidate_user_id = sqlx::query_scalar(
            "SELECT candidate_telegram_user_id FROM account_telegram_link_intents
             WHERE user_id=$1 AND session_digest=$2 AND expires_at>now()",
        )
        .bind(user_id)
        .bind(session_digest.as_slice())
        .fetch_optional(self.pool())
        .await?
        .flatten();
        Ok(TelegramLinkState {
            linked_user_id,
            candidate_user_id,
        })
    }

    /// Password step-up happens in the bounded web worker before this call.
    /// The database rechecks the exact credential and live initiating session.
    pub async fn begin_telegram_link(
        &self,
        user_id: i64,
        session_digest: &[u8; 32],
        expected_hash: &str,
        nonce_digest: &[u8; 32],
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if current.as_deref() != Some(expected_hash) {
            return Ok(false);
        }
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE user_id=$1 AND token_hash=$2
             AND revoked_at IS NULL AND expires_at>now() FOR UPDATE",
        )
        .bind(user_id)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO account_telegram_link_intents
             (user_id,token_hash,session_digest,credential_fingerprint)
             VALUES($1,$2,$3,$4)
             ON CONFLICT(user_id) DO UPDATE SET token_hash=EXCLUDED.token_hash,
             session_digest=EXCLUDED.session_digest,
             credential_fingerprint=EXCLUDED.credential_fingerprint,
             candidate_telegram_user_id=NULL,candidate_private_chat_id=NULL,
             created_at=now(),expires_at=now()+INTERVAL '5 minutes'",
        )
        .bind(user_id)
        .bind(nonce_digest.as_slice())
        .bind(session_digest.as_slice())
        .bind(session_token_digest(expected_hash).as_slice())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Bot-driven link claim is NOT a binding. The signed-in TOR owner
    /// must check the candidate and confirm in the initiating browser.
    pub async fn claim_telegram_link(
        &self,
        nonce_digest: &[u8; 32],
        telegram_user_id: i64,
        private_chat_id: i64,
    ) -> Result<bool, sqlx::Error> {
        if telegram_user_id <= 0 || private_chat_id != telegram_user_id {
            return Ok(false);
        }
        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE account_telegram_link_intents AS i
             SET candidate_telegram_user_id=$2,candidate_private_chat_id=$3
             WHERE i.token_hash=$1 AND i.expires_at>now()
               AND (i.candidate_telegram_user_id IS NULL OR i.candidate_telegram_user_id=$2)
               AND NOT EXISTS (SELECT 1 FROM account_telegram_bindings b
                               WHERE b.telegram_user_id=$2 AND b.user_id<>i.user_id)
             RETURNING i.user_id",
        )
        .bind(nonce_digest.as_slice())
        .bind(telegram_user_id)
        .bind(private_chat_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(updated.is_some())
    }

    /// Confirm the candidate and atomically replace any old Telegram binding
    /// and recovery codes. The plaintext backup codes are returned only by web.
    pub async fn finish_telegram_link(
        &self,
        user_id: i64,
        session_digest: &[u8; 32],
        code_hashes: &[[u8; 32]; 8],
    ) -> Result<Option<i64>, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(current) = current else {
            return Ok(None);
        };
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE user_id=$1 AND token_hash=$2
             AND revoked_at IS NULL AND expires_at>now() FOR UPDATE",
        )
        .bind(user_id)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            return Ok(None);
        }
        let pending: Option<(Option<i64>, Option<i64>, Vec<u8>)> = sqlx::query_as(
            "SELECT candidate_telegram_user_id,candidate_private_chat_id,
                    credential_fingerprint
             FROM account_telegram_link_intents
             WHERE user_id=$1 AND session_digest=$2 AND expires_at>now() FOR UPDATE",
        )
        .bind(user_id)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((Some(candidate), Some(chat), fingerprint)) = pending else {
            return Ok(None);
        };
        if candidate != chat || fingerprint != session_token_digest(&current) {
            return Ok(None);
        }
        sqlx::query("DELETE FROM account_telegram_bindings WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO account_telegram_bindings(user_id,telegram_user_id,private_chat_id)
             VALUES($1,$2,$3)",
        )
        .bind(user_id)
        .bind(candidate)
        .bind(chat)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM account_recovery_codes WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for digest in code_hashes {
            sqlx::query(
                "INSERT INTO account_recovery_codes(user_id,code_hash) VALUES($1,$2)",
            )
            .bind(user_id)
            .bind(digest.as_slice())
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("DELETE FROM account_telegram_link_intents WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(candidate))
    }

    /// A Telegram identity is proof only for ordinary, active accounts.
    /// Admin/moderator/author and other global roles require a separate factor.
    pub async fn issue_telegram_reset(
        &self,
        telegram_user_id: i64,
        private_chat_id: i64,
        reset_digest: &[u8; 32],
    ) -> Result<bool, sqlx::Error> {
        if telegram_user_id <= 0 || private_chat_id != telegram_user_id {
            return Ok(false);
        }
        let mut tx = self.pool().begin().await?;
        let account: Option<(i64, String)> = sqlx::query_as(
            "SELECT u.id,u.password_hash FROM account_telegram_bindings b
             JOIN users u ON u.id=b.user_id
             WHERE b.telegram_user_id=$1 AND b.private_chat_id=$2
               AND u.account_state='active'
               AND NOT EXISTS (
                 SELECT 1 FROM account_roles r WHERE r.user_id=u.id AND r.is_active
               )
               AND NOT EXISTS (SELECT 1 FROM books b WHERE b.owner_user_id=u.id)
               AND NOT EXISTS (SELECT 1 FROM book_members bm
                               WHERE bm.user_id=u.id AND bm.is_active)
             FOR UPDATE OF u",
        )
        .bind(telegram_user_id)
        .bind(private_chat_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, hash)) = account else {
            return Ok(false);
        };
        sqlx::query(
            "UPDATE password_reset_tokens SET consumed_at=now()
             WHERE user_id=$1 AND consumed_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO password_reset_tokens
             (token_hash,user_id,credential_fingerprint,source,expires_at)
             VALUES($1,$2,$3,'telegram',now()+INTERVAL '10 minutes')",
        )
        .bind(reset_digest.as_slice())
        .bind(user_id)
        .bind(session_token_digest(&hash).as_slice())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn consume_backup_code(
        &self,
        normalized_email: &str,
        code_digest: &[u8; 32],
        reset_digest: &[u8; 32],
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let account: Option<(i64, String)> = sqlx::query_as(
            "SELECT u.id,u.password_hash FROM users u WHERE u.email_normalized=$1
             AND u.account_state='active'
             AND NOT EXISTS (SELECT 1 FROM account_roles r
                             WHERE r.user_id=u.id AND r.is_active)
               AND NOT EXISTS (SELECT 1 FROM books b WHERE b.owner_user_id=u.id)
               AND NOT EXISTS (SELECT 1 FROM book_members bm
                               WHERE bm.user_id=u.id AND bm.is_active)
             FOR UPDATE OF u",
        )
        .bind(normalized_email)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, hash)) = account else {
            return Ok(false);
        };
        let consumed: Option<i64> = sqlx::query_scalar(
            "UPDATE account_recovery_codes SET consumed_at=now()
             WHERE user_id=$1 AND code_hash=$2 AND consumed_at IS NULL
             RETURNING user_id",
        )
        .bind(user_id)
        .bind(code_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if consumed.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE password_reset_tokens SET consumed_at=now()
             WHERE user_id=$1 AND consumed_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO password_reset_tokens
             (token_hash,user_id,credential_fingerprint,source,expires_at)
             VALUES($1,$2,$3,'backup',now()+INTERVAL '10 minutes')",
        )
        .bind(reset_digest.as_slice())
        .bind(user_id)
        .bind(session_token_digest(&hash).as_slice())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn mark_telegram_update(&self, update_id: i64) -> Result<bool, sqlx::Error> {
        if update_id < 0 {
            return Ok(false);
        }
        let receipt: Option<i64> = sqlx::query_scalar(
            "INSERT INTO account_telegram_bot_updates(update_id) VALUES($1)
             ON CONFLICT DO NOTHING RETURNING update_id",
        )
        .bind(update_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(receipt.is_some())
    }

    pub async fn revoke_telegram_reset(&self, digest: &[u8; 32]) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE password_reset_tokens SET consumed_at=now()
             WHERE token_hash=$1 AND source='telegram' AND consumed_at IS NULL",
        )
        .bind(digest.as_slice())
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
