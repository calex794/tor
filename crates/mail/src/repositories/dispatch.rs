//! Persistence adapter for one-shot dispatch. Driver errors remain inside
//! repositories; no raw database error or private payload crosses the boundary.
use super::{
    launch::PgMailLaunch, postbox_feedback::PgPostboxFeedback, quota::PgMailQuota, PgMailQueue,
};
use crate::domain::{
    dispatch::{DeferOutcome, DeferReason},
    launch::LaunchSnapshot,
    postbox_profile::SealedProfile,
    quota::{QuotaDecision, QuotaKeys},
    suppression::AttemptRef,
    MailClaim, MailOutcome,
};
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchStoreError;

impl From<sqlx::Error> for DispatchStoreError {
    fn from(_: sqlx::Error) -> Self {
        Self
    }
}

#[derive(Clone)]
pub struct PgDispatchStore {
    queue: PgMailQueue,
    quota: PgMailQuota,
    launch: PgMailLaunch,
    feedback: PgPostboxFeedback,
}

impl PgDispatchStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            queue: PgMailQueue::new(pool.clone()),
            quota: PgMailQuota::new(pool.clone()),
            launch: PgMailLaunch::new(pool.clone()),
            feedback: PgPostboxFeedback::new(pool),
        }
    }

    pub(crate) async fn snapshot(&self) -> Result<Option<LaunchSnapshot>, DispatchStoreError> {
        Ok(self.launch.snapshot().await?)
    }

    pub(crate) async fn dispatch_profile(
        &self,
        expected: &LaunchSnapshot,
    ) -> Result<Option<SealedProfile>, DispatchStoreError> {
        Ok(self.launch.dispatch_profile(expected).await?)
    }

    pub(crate) async fn claim_next(&self) -> Result<Option<MailClaim>, DispatchStoreError> {
        Ok(self.queue.claim_next().await?)
    }

    pub(crate) async fn reserve_for_dispatch(
        &self,
        claim: MailClaim,
        keys: QuotaKeys,
        expected: &LaunchSnapshot,
    ) -> Result<QuotaDecision, DispatchStoreError> {
        Ok(self
            .quota
            .reserve_for_dispatch(claim, keys, expected)
            .await?)
    }

    pub(crate) async fn prepare_for_dispatch(
        &self,
        claim: MailClaim,
        identity: &str,
        keys: QuotaKeys,
        expected: &LaunchSnapshot,
    ) -> Result<Option<String>, DispatchStoreError> {
        Ok(self
            .feedback
            .prepare_for_dispatch(claim, identity, keys, expected)
            .await?)
    }

    pub(crate) async fn bind_accepted_message(
        &self,
        attempt: AttemptRef,
        identity: &str,
        id: &str,
    ) -> Result<bool, DispatchStoreError> {
        Ok(self
            .feedback
            .bind_accepted_message(attempt, identity, id)
            .await?)
    }

    pub(crate) async fn defer_unsubmitted(
        &self,
        claim: MailClaim,
        reason: DeferReason,
    ) -> Result<DeferOutcome, DispatchStoreError> {
        Ok(self.queue.defer_unsubmitted(claim, reason).await?)
    }

    pub(crate) async fn finish(
        &self,
        claim: MailClaim,
        outcome: MailOutcome,
    ) -> Result<bool, DispatchStoreError> {
        Ok(self.queue.finish(claim, outcome).await?)
    }
}
