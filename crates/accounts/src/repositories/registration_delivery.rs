//! Registration confirmation resolves only the original committed signup.
//! The existing signer still verifies current mailbox and captured IP binding.
use super::{email_ownership::MailboxCandidate, registration_mail::digest, PgAccounts};
use tor_contracts::{
    auth::UserId,
    mail::{MailIntent, MailPurpose, RegistrationMailEvidence, SignupMailSource},
    paid_delivery::PositiveId,
};

fn invalid() -> sqlx::Error {
    sqlx::Error::Protocol("invalid registration mail reference".to_owned())
}

fn intent(id: i64) -> Result<MailIntent, sqlx::Error> {
    Ok(MailIntent {
        purpose: MailPurpose::RegistrationConfirmation,
        event_id: PositiveId::new(id).map_err(|_| invalid())?,
        recipient: UserId::new(id).map_err(|_| invalid())?,
    })
}

fn initial_signup(request: MailIntent) -> bool {
    request.purpose == MailPurpose::RegistrationConfirmation
        && request.event_id.get() == request.recipient.get()
}

type SourceRow = (String, i64, Vec<u8>, Option<Vec<u8>>, Vec<u8>);

impl PgAccounts {
    /// Bounded restart-safe handoff: the caller enqueues each reference
    /// idempotently and acknowledges only after a committed queue receipt.
    /// No pending mail is created for legacy or already verified accounts.
    pub async fn pending_registration_mail_intents(
        &self,
        limit: u32,
    ) -> Result<Vec<MailIntent>, sqlx::Error> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT s.user_id FROM account_registration_mail_sources s
             JOIN users u ON u.id=s.user_id
             WHERE s.forwarded_at IS NULL AND u.account_state='active'
               AND s.created_at>clock_timestamp()-interval '24 hours'
               AND NOT EXISTS (
                 SELECT 1 FROM account_email_ownership p
                 WHERE p.user_id=u.id AND p.verified_at IS NOT NULL
               )
             ORDER BY s.created_at,s.user_id LIMIT $1",
        )
        .bind(i64::from(limit.min(100)))
        .fetch_all(self.pool())
        .await?;
        ids.into_iter().map(intent).collect()
    }

    /// The original signup is unique per account. No browser event IDs,
    /// new resend events or acknowledgement of a different purpose.
    pub async fn mark_registration_mail_enqueued(
        &self,
        request: MailIntent,
    ) -> Result<bool, sqlx::Error> {
        if !initial_signup(request) {
            return Ok(false);
        }
        let result = sqlx::query(
            "UPDATE account_registration_mail_sources SET forwarded_at=clock_timestamp()
             WHERE user_id=$1 AND forwarded_at IS NULL
               AND created_at>clock_timestamp()-interval '24 hours'",
        )
        .bind(request.recipient.get())
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Read source evidence and the exact active mailbox in one statement.
    /// A captured signup is not a send permit. A future dispatch must use
    /// the captured evidence with the quota signer and repeat this check
    /// at the final reserved-proof/correlation transaction.
    pub async fn resolve_registration_mail(
        &self,
        request: MailIntent,
    ) -> Result<Option<(MailboxCandidate, RegistrationMailEvidence)>, sqlx::Error> {
        if !initial_signup(request) {
            return Ok(None);
        }
        let row: Option<SourceRow> = sqlx::query_as(
            "SELECT u.email_normalized,u.mail_ownership_revision,
                    s.recipient_key,s.registration_ip_key,s.key_binding
             FROM account_registration_mail_sources s JOIN users u ON u.id=s.user_id
             WHERE s.user_id=$1 AND u.account_state='active'
               AND s.created_at>clock_timestamp()-interval '24 hours'
               AND NOT EXISTS (
                 SELECT 1 FROM account_email_ownership p
                 WHERE p.user_id=u.id AND p.verified_at IS NOT NULL
               )",
        )
        .bind(request.recipient.get())
        .fetch_optional(self.pool())
        .await?;
        let Some((address, revision, recipient, ip, binding)) = row else {
            return Ok(None);
        };
        let Some(candidate) =
            MailboxCandidate::from_account(request.recipient, &address, revision)
        else {
            return Ok(None);
        };
        Ok(Some((
            candidate,
            RegistrationMailEvidence {
                user_id: request.recipient,
                source: SignupMailSource {
                    recipient_key: digest(recipient)?,
                    registration_ip_key: ip.map(digest).transpose()?,
                    key_binding: digest(binding)?,
                },
            },
        )))
    }
}
