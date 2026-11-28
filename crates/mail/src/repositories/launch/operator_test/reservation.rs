//! Atomic test-only reservation in the shared queue, quota and cost ledgers.
use super::{checked_context, load, PgMailLaunch};
use crate::{
    domain::{
        launch::OperatorTestGrant,
        quota::{identity::QuotaKeySigner, QuotaDecision},
    },
    repositories::{
        cost::record_ledger_reservation, postbox_profile::ProfileError,
        quota::ledger::reserve_ledger,
    },
};
use tor_contracts::mail::MailPurpose;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorTestReserveError {
    Profile(ProfileError),
    Quota(QuotaDecision),
}

impl From<ProfileError> for OperatorTestReserveError {
    fn from(error: ProfileError) -> Self {
        Self::Profile(error)
    }
}

impl From<sqlx::Error> for OperatorTestReserveError {
    fn from(_: sqlx::Error) -> Self {
        Self::Profile(ProfileError::Unavailable)
    }
}

/// Only a successful committed reservation creates this receipt. It is not
/// an SMTP permission: the final exact DB/local witness is a separate step.
pub struct OperatorTestReservation {
    outbox_id: i64,
}

impl OperatorTestReservation {
    pub fn outbox_id(&self) -> i64 {
        self.outbox_id
    }

    pub fn generation(&self) -> i32 {
        1
    }
}

impl PgMailLaunch {
    /// Never queue a pending operator test or retry an ambiguous commit.
    /// No browser-supplied mailbox, no connection or submission here.
    pub async fn reserve_operator_test(
        &self,
        grant: &OperatorTestGrant,
        allowlisted_recipient: &str,
        signer: &QuotaKeySigner,
    ) -> Result<OperatorTestReservation, OperatorTestReserveError> {
        use OperatorTestReserveError::Quota;
        let mut tx = self.pool.begin().await?;
        let (before, context) = checked_context(&mut tx, grant, allowlisted_recipient, false).await?;
        let keys = signer
            .keys(context.recipient(), None)
            .map_err(|_| ProfileError::Invalid)?;
        let digest = before.activation_digest.ok_or(ProfileError::Conflict)?;
        let key_binding = sqlx::query_scalar::<_, Option<Vec<u8>>>(
            "SELECT key_binding FROM mail_send_budget_guard WHERE singleton",
        )
        .fetch_one(&mut *tx)
        .await?;
        // A private purpose, not constructible as a public MailIntent.
        // The activation revision is a unique durable source reference.
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO mail_outbox \
             (purpose,event_id,recipient_user_id,status,generation,lease_until) \
             VALUES('operator_test',$1,$2,'leased',1,clock_timestamp()+interval '120 seconds') \
             ON CONFLICT(purpose,event_id,recipient_user_id) DO NOTHING RETURNING id",
        )
        .bind(before.policy_revision())
        .bind(grant.actor())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(id) = id else {
            return Err(Quota(QuotaDecision::AlreadyReserved));
        };
        let decision = reserve_ledger(
            &mut tx,
            key_binding,
            id,
            1,
            "operator_test",
            keys,
            Some(&digest),
        )
        .await?;
        if decision != QuotaDecision::Reserved {
            return Err(Quota(decision));
        }
        // Nonessential tests must respect the news-pause threshold and
        // transactional reserve; the actual ledger purpose stays operator_test.
        let cost = record_ledger_reservation(&mut tx, id, 1, MailPurpose::ProjectNews).await?;
        if cost != QuotaDecision::Reserved {
            return Err(Quota(cost));
        }
        let current = load(&mut tx).await?.ok_or(ProfileError::Conflict)?;
        if !grant.matches_reserved(&current) {
            return Err(ProfileError::Conflict.into());
        }
        tx.commit().await?;
        // A late commit is still charged. Never retry or refund it.
        if !grant.matches_reserved(&current) {
            return Err(ProfileError::Conflict.into());
        }
        Ok(OperatorTestReservation { outbox_id: id })
    }
}
