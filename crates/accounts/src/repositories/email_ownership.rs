//! Account-owned mailbox verification. Never creates a login session or sends mail.
use super::PgAccounts;
use crate::{domain::email::delivery_address, services::security::session_token_digest};
use tor_contracts::auth::UserId;

/// Private delivery candidate. No Debug/serialization and no client-supplied mailbox.
pub struct MailboxCandidate {
    user_id: UserId,
    address_sha256: [u8; 32],
    revision: i64,
    transport: String,
}

impl MailboxCandidate {
    pub(super) fn from_account(user_id: UserId, address: &str, revision: i64) -> Option<Self> {
        Some(Self {
            user_id,
            address_sha256: session_token_digest(address),
            revision,
            transport: delivery_address(address).ok()?,
        })
    }

    pub fn transport_address(&self) -> &str {
        &self.transport
    }

    /// The canonical worker binds proof issuance to this non-reused revision.
    pub fn revision(&self) -> i64 {
        self.revision
    }

    /// Compare only private, account-owned facts, not a posted address.
    pub fn same_mailbox(&self, other: &Self) -> bool {
        self.user_id == other.user_id
            && self.revision == other.revision
            && self.address_sha256 == other.address_sha256
            && self.transport == other.transport
    }
}

impl PgAccounts {
    /// Resolve only the active account's current address. This is not ownership proof.
    pub async fn mailbox_candidate(
        &self,
        user_id: UserId,
    ) -> Result<Option<MailboxCandidate>, sqlx::Error> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT email_normalized,mail_ownership_revision
             FROM users WHERE id=$1 AND account_state='active'",
        )
        .bind(user_id.get())
        .fetch_optional(self.pool())
        .await?;
        Ok(row.and_then(|(address, revision)| {
            MailboxCandidate::from_account(user_id, &address, revision)
        }))
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
        let current: Option<(String, i64)> = sqlx::query_as(
            "SELECT email_normalized,mail_ownership_revision
             FROM users WHERE id=$1 AND account_state='active' FOR UPDATE",
        )
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((current, revision)) = current else {
            return Ok(false);
        };
        let consumed: Option<i64> = sqlx::query_scalar(
            "UPDATE account_email_ownership
             SET verified_at=statement_timestamp(),challenge_sha256=NULL,expires_at=NULL
             WHERE user_id=$1 AND address_sha256=$2 AND challenge_sha256=$3
               AND mailbox_revision=$4 AND verified_at IS NULL AND expires_at>statement_timestamp()
             RETURNING user_id",
        )
        .bind(actor.get())
        .bind(session_token_digest(&current).as_slice())
        .bind(challenge_sha256.as_slice())
        .bind(revision)
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
               AND p.address_sha256=sha256(convert_to(u.email_normalized,'UTF8'))
               AND p.mailbox_revision=u.mail_ownership_revision",
        )
        .bind(actor.get())
        .fetch_optional(self.pool())
        .await?;
        Ok(address.and_then(|address| delivery_address(&address).ok()))
    }
}
