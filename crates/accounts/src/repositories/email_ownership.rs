//! Account-owned mailbox verification. Never creates a login session or sends mail.
use super::PgAccounts;
use crate::{domain::email::delivery_address, services::security::session_token_digest};
use tor_contracts::auth::UserId;

/// Private delivery candidate. No Debug/serialization and no client-supplied mailbox.
pub struct MailboxCandidate {
    user_id: UserId,
    address_sha256: [u8; 32],
    transport: String,
}

impl MailboxCandidate {
    pub fn transport_address(&self) -> &str {
        &self.transport
    }
}

impl PgAccounts {
    /// Resolve only the active account's current address. This is not ownership proof.
    pub async fn mailbox_candidate(
        &self,
        user_id: UserId,
    ) -> Result<Option<MailboxCandidate>, sqlx::Error> {
        let address: Option<String> = sqlx::query_scalar(
            "SELECT email_normalized FROM users WHERE id=$1 AND account_state='active'",
        )
        .bind(user_id.get())
        .fetch_optional(self.pool())
        .await?;
        Ok(address.and_then(|address| {
            let transport = delivery_address(&address).ok()?;
            Some(MailboxCandidate {
                user_id,
                address_sha256: session_token_digest(&address),
                transport,
            })
        }))
    }

    /// Internal proof storage primitive. The future common dispatcher must
    /// bind issuance to its reserved source and durable correlation transaction.
    /// No HTTP handler or production caller is wired by this slice.
    /// Keep the raw random challenge in memory only. Reissue at most once
    /// per minute; an already verified mailbox needs no new challenge.
    pub async fn issue_email_ownership_proof(
        &self,
        expected: &MailboxCandidate,
        challenge_sha256: &[u8; 32],
    ) -> Result<bool, sqlx::Error> {
        if *challenge_sha256 == [0; 32] {
            return Ok(false);
        }
        let mut tx = self.pool().begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT email_normalized FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(expected.user_id.get())
        .fetch_optional(&mut *tx)
        .await?;
        if !current
            .is_some_and(|address| session_token_digest(&address) == expected.address_sha256)
        {
            return Ok(false);
        }
        let saved: Option<i64> = sqlx::query_scalar(
            "INSERT INTO account_email_ownership AS p
                (user_id,address_sha256,challenge_sha256,issued_at,expires_at)
             VALUES($1,$2,$3,statement_timestamp(),statement_timestamp()+interval '30 minutes')
             ON CONFLICT(user_id) DO UPDATE SET
                 address_sha256=EXCLUDED.address_sha256,
                 challenge_sha256=EXCLUDED.challenge_sha256,
                 issued_at=EXCLUDED.issued_at,expires_at=EXCLUDED.expires_at
             WHERE p.verified_at IS NULL
               AND p.issued_at<=statement_timestamp()-interval '60 seconds'
             RETURNING user_id",
        )
        .bind(expected.user_id.get())
        .bind(expected.address_sha256.as_slice())
        .bind(challenge_sha256.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(saved.is_some())
    }

    /// Only a live authenticated actor may consume its own proof. The
    /// caller resolves that actor from the session, not a posted user ID.
    /// Confirmation is single-use and has no authentication side effect.
    pub async fn consume_email_ownership_proof(
        &self,
        actor: UserId,
        challenge_sha256: &[u8; 32],
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT email_normalized FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(current) = current else {
            return Ok(false);
        };
        let consumed: Option<i64> = sqlx::query_scalar(
            "UPDATE account_email_ownership
             SET verified_at=statement_timestamp(),challenge_sha256=NULL,expires_at=NULL
             WHERE user_id=$1 AND address_sha256=$2 AND challenge_sha256=$3
               AND verified_at IS NULL AND expires_at>statement_timestamp()
             RETURNING user_id",
        )
        .bind(actor.get())
        .bind(session_token_digest(&current).as_slice())
        .bind(challenge_sha256.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(consumed.is_some())
    }

    /// For future support notifications. A saved address, DNS routing or
    /// reader-news subscription never substitutes for mailbox ownership.
    pub async fn verified_mail_delivery_address(
        &self,
        actor: UserId,
    ) -> Result<Option<String>, sqlx::Error> {
        let address: Option<String> = sqlx::query_scalar(
            "SELECT u.email_normalized FROM users u
             JOIN account_email_ownership p ON p.user_id=u.id
             WHERE u.id=$1 AND u.account_state='active' AND p.verified_at IS NOT NULL
               AND p.address_sha256=sha256(convert_to(u.email_normalized,'UTF8'))",
        )
        .bind(actor.get())
        .fetch_optional(self.pool())
        .await?;
        Ok(address.and_then(|address| delivery_address(&address).ok()))
    }
}
