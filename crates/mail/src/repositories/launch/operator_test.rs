//! Exact read-only recipient/profile resolution for a one-shot operator test.
use super::{load, load_profile, lock_guard, PgMailLaunch};
use crate::{
    domain::{launch::OperatorTestGrant, postbox_profile::SealedProfile},
    repositories::postbox_profile::{operator, ProfileError},
};

/// Private transient context: no Debug, serialization, or HTTP form constructor.
pub struct OperatorTestContext {
    recipient: String,
    profile: SealedProfile,
}

impl OperatorTestContext {
    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    pub fn profile(&self) -> &SealedProfile {
        &self.profile
    }
}

impl PgMailLaunch {
    /// The caller must pass a server-configured, independently verified
    /// destination, never browser input. This check is not mailbox ownership
    /// proof. Repeat with the eventual exact shared-ledger send-start gate.
    pub async fn operator_test_context(
        &self,
        grant: &OperatorTestGrant,
        allowlisted_recipient: &str,
    ) -> Result<OperatorTestContext, ProfileError> {
        let mut tx = self.pool.begin().await?;
        let actor = operator(&mut tx, &grant.session).await?;
        if actor != grant.actor() {
            return Err(ProfileError::Forbidden);
        }
        lock_guard(&mut tx).await?;
        let current = load(&mut tx).await?.ok_or(ProfileError::Conflict)?;
        if !grant.matches(&current) {
            return Err(ProfileError::Conflict);
        }
        let digest = current.activation_digest.ok_or(ProfileError::Conflict)?;
        let exact: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM mail_launch_activation_audit \
             WHERE new_revision=$1 AND intent_digest=$2 AND mode='operator_test' \
               AND actor_user_id=$3 AND test_user_id=$3)",
        )
        .bind(current.policy_revision())
        .bind(digest.as_slice())
        .bind(actor)
        .fetch_one(&mut *tx)
        .await?;
        if !exact {
            return Err(ProfileError::Conflict);
        }
        let recipient: String = sqlx::query_scalar(
            "SELECT email_normalized FROM users WHERE id=$1 AND account_state='active'",
        )
        .bind(actor)
        .fetch_one(&mut *tx)
        .await?;
        if recipient.is_empty()
            || recipient.len() > 254
            || recipient.trim() != recipient
            || recipient != allowlisted_recipient
        {
            return Err(ProfileError::Forbidden);
        }
        let profile = load_profile(&mut tx).await?;
        if !profile.valid() || &profile.intent_digest() != current.profile_digest() {
            return Err(ProfileError::Conflict);
        }
        tx.commit().await?;
        if !grant.matches(&current) {
            return Err(ProfileError::Conflict);
        }
        Ok(OperatorTestContext { recipient, profile })
    }
}
