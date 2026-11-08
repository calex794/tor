//! A/S004: browser-bound Telegram recovery. Bot callbacks are NOT reset tokens.
//! Unknown, unbound, and ineligible identities get decoy browser requests.
use super::PgAccounts;
use crate::services::security::session_token_digest;

#[derive(Debug)]
pub struct TelegramRecoveryView {
    pub match_code: String,
    pub status: String,
}

#[derive(Debug)]
pub struct TelegramRecoveryDispatch {
    pub id: i64,
    pub private_chat_id: i64,
    pub match_code: String,
}

impl PgAccounts {
    /// Same persisted HTTP path for valid and unknown emails. A request is
    /// readable only by the browser that holds its random HttpOnly cookie.
    pub async fn begin_browser_telegram_recovery(
        &self,
        normalized_email: Option<&str>,
        browser_digest: &[u8; 32],
        match_code: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let account: Option<(i64, i64, i64, String)> = if let Some(email) = normalized_email {
            sqlx::query_as(
                "SELECT u.id,b.telegram_user_id,b.private_chat_id,u.password_hash
                 FROM users u JOIN account_telegram_bindings b ON b.user_id=u.id
                 WHERE u.email_normalized=$1 AND u.account_state='active'
                 AND NOT EXISTS (SELECT 1 FROM account_roles r
                                 WHERE r.user_id=u.id AND r.is_active)
                 AND NOT EXISTS (SELECT 1 FROM books bk WHERE bk.owner_user_id=u.id)
                 AND NOT EXISTS (SELECT 1 FROM book_members bm
                                 WHERE bm.user_id=u.id AND bm.is_active)
                 FOR UPDATE OF u",
            )
            .bind(email)
            .fetch_optional(&mut *tx)
            .await?
        } else {
            None
        };
        if let Some((user_id, _, _, _)) = &account {
            // A later request invalidates previously approved attempts too.
            sqlx::query(
                "UPDATE account_telegram_recovery_requests SET status='denied'
                 WHERE user_id=$1 AND status IN ('pending','approved')",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        }
        let fingerprint = account
            .as_ref()
            .map(|(_, _, _, hash)| session_token_digest(hash).to_vec());
        sqlx::query(
            "INSERT INTO account_telegram_recovery_requests
             (user_id,telegram_user_id,private_chat_id,credential_fingerprint,
              browser_digest,match_code)
             VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(account.as_ref().map(|x| x.0))
        .bind(account.as_ref().map(|x| x.1))
        .bind(account.as_ref().map(|x| x.2))
        .bind(fingerprint.as_deref())
        .bind(browser_digest.as_slice())
        .bind(match_code)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn browser_telegram_recovery(
        &self,
        browser_digest: &[u8; 32],
    ) -> Result<Option<TelegramRecoveryView>, sqlx::Error> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT match_code,status FROM account_telegram_recovery_requests
             WHERE browser_digest=$1 AND expires_at>now()",
        )
        .bind(browser_digest.as_slice())
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|(match_code, status)| TelegramRecoveryView { match_code, status }))
    }

    /// Bounded outbox; a failed Telegram send leaves the record pending.
    /// No reset credential or browser proof ever leaves TOR.
    pub async fn pending_telegram_recovery_notifications(
        &self,
    ) -> Result<Vec<TelegramRecoveryDispatch>, sqlx::Error> {
        let rows: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT r.id,r.private_chat_id,r.match_code
             FROM account_telegram_recovery_requests r
             JOIN account_telegram_bindings b ON b.user_id=r.user_id
                  AND b.telegram_user_id=r.telegram_user_id
                  AND b.private_chat_id=r.private_chat_id
             JOIN users u ON u.id=r.user_id AND u.account_state='active'
             WHERE r.status='pending' AND r.expires_at>now()
               AND r.notified_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM account_roles roles
                               WHERE roles.user_id=u.id AND roles.is_active)
               AND NOT EXISTS (SELECT 1 FROM books bk WHERE bk.owner_user_id=u.id)
               AND NOT EXISTS (SELECT 1 FROM book_members bm
                               WHERE bm.user_id=u.id AND bm.is_active)
             ORDER BY r.id LIMIT 16",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, private_chat_id, match_code)| TelegramRecoveryDispatch {
                id,
                private_chat_id,
                match_code,
            })
            .collect())
    }

    pub async fn mark_telegram_recovery_notified(&self, request_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE account_telegram_recovery_requests SET notified_at=now()
             WHERE id=$1 AND status='pending' AND expires_at>now()
               AND notified_at IS NULL",
        )
        .bind(request_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Callback approval requires Telegram's authenticated from.id, an actual
    /// linked private chat, active account, and a still-pending exact request.
    pub async fn decide_telegram_recovery(
        &self,
        request_id: i64,
        telegram_user_id: i64,
        private_chat_id: i64,
        approve: bool,
    ) -> Result<bool, sqlx::Error> {
        if request_id <= 0 || telegram_user_id <= 0 || telegram_user_id != private_chat_id {
            return Ok(false);
        }
        let next = if approve { "approved" } else { "denied" };
        let changed: Option<i64> = sqlx::query_scalar(
            "UPDATE account_telegram_recovery_requests r SET status=$4
             WHERE r.id=$1 AND r.telegram_user_id=$2 AND r.private_chat_id=$3
               AND r.status='pending' AND r.expires_at>now()
               AND EXISTS (
                 SELECT 1 FROM account_telegram_bindings b JOIN users u ON u.id=b.user_id
                 WHERE b.user_id=r.user_id AND b.telegram_user_id=$2
                   AND b.private_chat_id=$3 AND u.account_state='active'
               )
               AND NOT EXISTS (SELECT 1 FROM account_roles roles
                               WHERE roles.user_id=r.user_id AND roles.is_active)
               AND NOT EXISTS (SELECT 1 FROM books bk WHERE bk.owner_user_id=r.user_id)
               AND NOT EXISTS (SELECT 1 FROM book_members bm
                               WHERE bm.user_id=r.user_id AND bm.is_active)
             RETURNING r.id",
        )
        .bind(request_id)
        .bind(telegram_user_id)
        .bind(private_chat_id)
        .bind(next)
        .fetch_optional(self.pool())
        .await?;
        Ok(changed.is_some())
    }

    /// An approved Telegram callback still cannot change a password. Only the
    /// originating browser may exchange its proof for a single-use reset token.
    /// Lock order follows A: user -> recovery request -> reset tokens.
    pub async fn exchange_browser_telegram_approval(
        &self,
        browser_digest: &[u8; 32],
        reset_digest: &[u8; 32],
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let user_id: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM account_telegram_recovery_requests
             WHERE browser_digest=$1 AND status='approved' AND expires_at>now()",
        )
        .bind(browser_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let Some(user_id) = user_id else {
            return Ok(false);
        };
        let current_hash: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users
             WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(hash) = current_hash else {
            return Ok(false);
        };
        let evidence: Option<(Vec<u8>, i64, i64)> = sqlx::query_as(
            "SELECT credential_fingerprint,telegram_user_id,private_chat_id
             FROM account_telegram_recovery_requests
             WHERE browser_digest=$1 AND user_id=$2 AND status='approved'
               AND expires_at>now() FOR UPDATE",
        )
        .bind(browser_digest.as_slice())
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((fingerprint, tg_id, chat_id)) = evidence else {
            return Ok(false);
        };
        if fingerprint != session_token_digest(&hash) || tg_id != chat_id {
            return Ok(false);
        }
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1 FROM account_telegram_bindings b
               WHERE b.user_id=$1 AND b.telegram_user_id=$2 AND b.private_chat_id=$3
                 AND NOT EXISTS (SELECT 1 FROM account_roles r
                                 WHERE r.user_id=$1 AND r.is_active)
                 AND NOT EXISTS (SELECT 1 FROM books bk WHERE bk.owner_user_id=$1)
                 AND NOT EXISTS (SELECT 1 FROM book_members bm
                                 WHERE bm.user_id=$1 AND bm.is_active)
             )",
        )
        .bind(user_id)
        .bind(tg_id)
        .bind(chat_id)
        .fetch_one(&mut *tx)
        .await?;
        if !allowed {
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
             VALUES($1,$2,$3,'telegram',now()+INTERVAL '10 minutes')",
        )
        .bind(reset_digest.as_slice())
        .bind(user_id)
        .bind(session_token_digest(&hash).as_slice())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE account_telegram_recovery_requests SET status='used'
             WHERE browser_digest=$1",
        )
        .bind(browser_digest.as_slice())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}
