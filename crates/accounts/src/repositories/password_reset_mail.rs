//! A-owned reset request source. No proof issuance, network I/O or mail queue writes.
use super::PgAccounts;
use crate::{domain::email::delivery_address, services::security::session_token_digest};
use tor_contracts::{
    auth::UserId,
    mail::{MailIntent, MailPurpose, PasswordResetMailEvidence, PasswordResetMailSource},
    paid_delivery::PositiveId,
};

fn invalid() -> sqlx::Error {
    sqlx::Error::Protocol("invalid password reset mail source".to_owned())
}

fn intent(user_id: i64, event_id: i64) -> Result<MailIntent, sqlx::Error> {
    Ok(MailIntent {
        purpose: MailPurpose::PasswordReset,
        event_id: PositiveId::new(event_id).map_err(|_| invalid())?,
        recipient: UserId::new(user_id).map_err(|_| invalid())?,
    })
}

fn digest(bytes: Vec<u8>) -> Result<[u8; 32], sqlx::Error> {
    bytes.try_into().map_err(|_| invalid())
}

type SourceRow = (String, String, Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<u8>);

impl PgAccounts {
    /// Internal trusted ingress only: source keys must come from the signer
    /// and verified peer, not the browser. The caller must not expose whether
    /// this returns Some or None in its HTTP acknowledgement.
    pub async fn capture_password_reset_mail_request(
        &self,
        normalized_email: &str,
        source: PasswordResetMailSource,
    ) -> Result<Option<MailIntent>, sqlx::Error> {
        if delivery_address(normalized_email).is_err() {
            return Ok(None);
        }
        let mut tx = self.pool().begin().await?;
        let account: Option<(i64, String)> = sqlx::query_as(
            "SELECT id,password_hash FROM users \
             WHERE email_normalized=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(normalized_email)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, password_hash)) = account else {
            return Ok(None);
        };
        // One source per account. A fresh request never invalidates a
        // previously issued reset proof. Use the clock after account locking.
        sqlx::query(
            "DELETE FROM account_password_reset_mail_requests WHERE user_id=$1 \
             AND created_at<=clock_timestamp()-interval '120 seconds'",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        let event_id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO account_password_reset_mail_requests \
             (user_id,credential_fingerprint,recipient_key,request_ip_key,key_binding) \
             VALUES($1,$2,$3,$4,$5) ON CONFLICT(user_id) DO NOTHING RETURNING id",
        )
        .bind(user_id)
        .bind(session_token_digest(&password_hash).as_slice())
        .bind(source.recipient_key.as_slice())
        .bind(source.request_ip_key.as_ref().map(|key| key.as_slice()))
        .bind(source.key_binding.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        event_id.map(|id| intent(user_id, id)).transpose()
    }

    /// The caller enqueues each reference idempotently and acknowledges only
    /// after a durable queue receipt. No long lock or distributed transaction.
    pub async fn pending_password_reset_mail_intents(
        &self,
        limit: u32,
    ) -> Result<Vec<MailIntent>, sqlx::Error> {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT user_id,id FROM account_password_reset_mail_requests \
             WHERE forwarded_at IS NULL \
             AND created_at>clock_timestamp()-interval '10 minutes' \
             ORDER BY id LIMIT $1",
        )
        .bind(i64::from(limit.min(100)))
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|(user_id, id)| intent(user_id, id))
            .collect()
    }

    /// Exact-reference acknowledgement; a late receipt cannot mark a newer
    /// request. An expired/replaced source must not authorize delivery.
    pub async fn mark_password_reset_mail_enqueued(
        &self,
        request: MailIntent,
    ) -> Result<bool, sqlx::Error> {
        if request.purpose != MailPurpose::PasswordReset {
            return Ok(false);
        }
        let result = sqlx::query(
            "UPDATE account_password_reset_mail_requests SET forwarded_at=clock_timestamp() \
             WHERE id=$1 AND user_id=$2 AND forwarded_at IS NULL \
             AND created_at>clock_timestamp()-interval '10 minutes'",
        )
        .bind(request.event_id.get())
        .bind(request.recipient.get())
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Resolve immediately before quota admission. The signer must still
    /// match the returned current transport address to the captured key.
    /// This read is not a send permit or permission to issue a reset proof.
    pub async fn resolve_password_reset_mail_request(
        &self,
        request: MailIntent,
    ) -> Result<Option<(PasswordResetMailEvidence, String)>, sqlx::Error> {
        if request.purpose != MailPurpose::PasswordReset {
            return Ok(None);
        }
        let row: Option<SourceRow> = sqlx::query_as(
            "SELECT u.email_normalized,u.password_hash,r.credential_fingerprint, \
             r.recipient_key,r.request_ip_key,r.key_binding \
             FROM account_password_reset_mail_requests r JOIN users u ON u.id=r.user_id \
             WHERE r.id=$1 AND r.user_id=$2 AND u.account_state='active' \
             AND r.created_at>clock_timestamp()-interval '10 minutes'",
        )
        .bind(request.event_id.get())
        .bind(request.recipient.get())
        .fetch_optional(self.pool())
        .await?;
        let Some((email, hash, fingerprint, recipient, ip, binding)) = row else {
            return Ok(None);
        };
        if fingerprint.as_slice() != session_token_digest(&hash).as_slice() {
            return Ok(None);
        }
        let Ok(address) = delivery_address(&email) else {
            return Ok(None);
        };
        Ok(Some((
            PasswordResetMailEvidence {
                intent: request,
                source: PasswordResetMailSource {
                    recipient_key: digest(recipient)?,
                    request_ip_key: ip.map(digest).transpose()?,
                    key_binding: digest(binding)?,
                },
            },
            address,
        )))
    }

    pub(super) async fn prune_password_reset_mail_requests(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "WITH batch AS (SELECT id FROM account_password_reset_mail_requests \
             WHERE created_at<=clock_timestamp()-interval '10 minutes' \
             ORDER BY created_at,id LIMIT 1000 FOR UPDATE SKIP LOCKED) \
             DELETE FROM account_password_reset_mail_requests r USING batch b WHERE r.id=b.id",
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
