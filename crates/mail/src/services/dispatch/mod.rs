//! One-shot dispatch. No automatic startup, retry loop or test-mode sending.
mod policy;
mod source;

pub use source::{DispatchPreflight, DispatchSource, PreflightFailure, ResolvedMail, SourceFailure};
use super::{
    launch::{LaunchError, LaunchFence, LaunchScope},
    postbox::{self, PostboxCredentials, PostboxTransport, Submission},
};
use crate::{
    domain::{
        dispatch::{DeferOutcome, DeferReason},
        launch::LaunchSnapshot,
        postbox_profile::ProfileMasterKey,
        quota::identity::QuotaKeySigner,
        suppression::AttemptRef,
        MailClaim, MailOutcome,
    },
    repositories::dispatch::{DispatchStoreError, PgDispatchStore},
};
use policy::Denial;
use std::time::Duration;
use tokio::time::timeout;

const ATTEMPT_LIMIT: Duration = Duration::from_secs(90);
const SOURCE_LIMIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchOutcome {
    Stopped,
    Busy,
    Idle,
    Deferred(DeferReason),
    Exhausted,
    Finished(MailOutcome),
    LeaseLost,
    Unavailable,
}

/// All instances must share the application's one process-local LaunchFence.
/// A separate instance of the worker is not a second permit to send.
pub struct DispatchWorker<'a> {
    fence: &'a LaunchFence,
    signer: &'a QuotaKeySigner,
    master: &'a ProfileMasterKey,
    store: PgDispatchStore,
}

impl<'a> DispatchWorker<'a> {
    pub fn new(
        store: PgDispatchStore,
        fence: &'a LaunchFence,
        signer: &'a QuotaKeySigner,
        master: &'a ProfileMasterKey,
    ) -> Self {
        Self {
            fence,
            signer,
            master,
            store,
        }
    }

    /// At most one claim and one SMTP submission. The 90-second total
    /// deadline is shorter than the 120-second queue lease. On cancellation,
    /// database failure or timeout, leave any outstanding lease quarantinable
    /// by recovery; never assume a transaction or SMTP submission failed.
    pub async fn run_one(
        &self,
        source: &impl DispatchSource,
        preflight: &impl DispatchPreflight,
    ) -> DispatchOutcome {
        let mut slot = match self.fence.try_dispatch_run() {
            Ok(Some(slot)) => slot,
            Ok(None) => return DispatchOutcome::Busy,
            Err(LaunchError::Stopped) => return DispatchOutcome::Stopped,
            Err(_) => return DispatchOutcome::Unavailable,
        };
        let outcome = match timeout(ATTEMPT_LIMIT, self.run_inner(source, preflight)).await {
            Ok(Ok(outcome)) => outcome,
            _ => {
                self.stop();
                DispatchOutcome::Unavailable
            }
        };
        slot.complete();
        outcome
    }

    async fn run_inner(
        &self,
        source: &impl DispatchSource,
        preflight: &impl DispatchPreflight,
    ) -> Result<DispatchOutcome, DispatchStoreError> {
        let Some(initial) = self.permitted_snapshot().await? else {
            return Ok(DispatchOutcome::Stopped);
        };
        let Some(profile) = self.store.dispatch_profile(&initial).await? else {
            self.stop();
            return Ok(DispatchOutcome::Stopped);
        };
        let Ok(secrets) = self.master.open(&profile) else {
            self.stop();
            return Ok(DispatchOutcome::Unavailable);
        };
        let Ok(transport) = PostboxTransport::new(profile.identity().sender()) else {
            self.stop();
            return Ok(DispatchOutcome::Unavailable);
        };
        let Ok(credentials) =
            PostboxCredentials::new(secrets.access().to_owned(), secrets.secret().to_owned())
        else {
            self.stop();
            return Ok(DispatchOutcome::Unavailable);
        };
        drop(secrets);
        let Some(claim) = self.store.claim_next().await? else {
            return Ok(DispatchOutcome::Idle);
        };
        let mail = match timeout(SOURCE_LIMIT, source.resolve(claim.intent)).await {
            Ok(Ok(mail)) => mail,
            Ok(Err(SourceFailure::Cancelled)) => {
                return self.finish(claim, MailOutcome::Cancelled).await;
            }
            _ => return self.defer(claim, DeferReason::SourceUnavailable).await,
        };
        let Some(keys) = mail.keys(claim.intent, self.signer) else {
            return self.finish(claim, MailOutcome::Cancelled).await;
        };
        if postbox::validate_content(&mail.recipient, &mail.subject, &mail.body).is_err() {
            return self.finish(claim, MailOutcome::Failed).await;
        }
        match timeout(SOURCE_LIMIT, preflight.check(&mail.recipient)).await {
            Ok(Ok(())) => {}
            Ok(Err(PreflightFailure::InvalidRecipient)) => {
                return self.finish(claim, MailOutcome::Cancelled).await;
            }
            _ => return self.defer(claim, DeferReason::DnsUnavailable).await,
        }
        let Ok(session) = transport.connect(&credentials).await else {
            return self.defer(claim, DeferReason::ProviderUnavailable).await;
        };
        drop(credentials);
        match timeout(SOURCE_LIMIT, source.resolve(claim.intent)).await {
            Ok(Ok(current)) if mail.same(&current) => {}
            Ok(Ok(_) | Err(SourceFailure::Cancelled)) => {
                return self.finish(claim, MailOutcome::Cancelled).await;
            }
            _ => return self.defer(claim, DeferReason::SourceUnavailable).await,
        }
        let Some(before) = self.permitted_snapshot().await? else {
            return self.defer(claim, DeferReason::BudgetPaused).await;
        };
        if before != initial {
            self.stop();
            return self.defer(claim, DeferReason::BudgetPaused).await;
        }
        let decision = self.store.reserve_for_dispatch(claim, keys, &before).await?;
        if let Some(denial) = policy::denial(decision) {
            return self.handle_denial(claim, denial).await;
        }

        // From here no path may defer or refund the committed reservation.
        let Some(reserved) = self.store.snapshot().await? else {
            return self.cancel_reserved(claim).await;
        };
        if !matches!(
            timeout(SOURCE_LIMIT, source.resolve(claim.intent)).await,
            Ok(Ok(current)) if mail.same(&current)
        ) {
            return self.cancel_reserved(claim).await;
        }
        let identity = profile.identity().identity_id();
        let Some(marker) = self
            .store
            .prepare_for_dispatch(claim, identity, keys, &reserved)
            .await?
        else {
            return self.cancel_reserved(claim).await;
        };
        let Ok(message) = transport.prepare(&mail.recipient, &mail.subject, &mail.body, &marker)
        else {
            return self.cancel_reserved(claim).await;
        };
        let Some(current) = self.store.snapshot().await? else {
            return self.cancel_reserved(claim).await;
        };
        if current != reserved
            || self
                .fence
                .consume_start(&current, LaunchScope::Dispatch)
                .is_err()
        {
            return self.cancel_reserved(claim).await;
        }
        // Local consume_start is the admission boundary; a stop cannot
        // revoke a submission already admitted. No await before submit.
        let submission = session.submit(message).await;
        let outcome = match submission {
            Submission::Accepted {
                provider_message_id,
            } => {
                if let Some(id) = provider_message_id {
                    let attempt = AttemptRef {
                        outbox_id: claim.id,
                        generation: claim.generation,
                    };
                    if !self
                        .store
                        .bind_accepted_message(attempt, identity, &id)
                        .await?
                    {
                        self.stop();
                    }
                }
                MailOutcome::Accepted
            }
            Submission::TemporaryRejected | Submission::PermanentRejected => MailOutcome::Failed,
            Submission::InvalidPreparedSender => {
                self.stop();
                MailOutcome::Failed
            }
            Submission::Uncertain => {
                self.stop();
                MailOutcome::Uncertain
            }
        };
        self.finish(claim, outcome).await
    }

    async fn permitted_snapshot(&self) -> Result<Option<LaunchSnapshot>, DispatchStoreError> {
        let Some(snapshot) = self.store.snapshot().await? else {
            self.stop();
            return Ok(None);
        };
        if self
            .fence
            .permit_reservation(&snapshot, LaunchScope::Dispatch)
            .is_err()
        {
            return Ok(None);
        }
        Ok(Some(snapshot))
    }

    fn stop(&self) {
        let _ = self.fence.stop();
    }

    async fn cancel_reserved(
        &self,
        claim: MailClaim,
    ) -> Result<DispatchOutcome, DispatchStoreError> {
        self.stop();
        self.finish(claim, MailOutcome::Cancelled).await
    }

    async fn handle_denial(
        &self,
        claim: MailClaim,
        denial: Denial,
    ) -> Result<DispatchOutcome, DispatchStoreError> {
        match denial {
            Denial::Defer(reason) => self.defer(claim, reason).await,
            Denial::Finish(outcome) => self.finish(claim, outcome).await,
            Denial::StopAndDefer => {
                self.stop();
                self.defer(claim, DeferReason::BudgetPaused).await
            }
            Denial::StopAndFinish(outcome) => {
                self.stop();
                self.finish(claim, outcome).await
            }
            Denial::LeaseLost => Ok(DispatchOutcome::LeaseLost),
        }
    }

    async fn defer(
        &self,
        claim: MailClaim,
        reason: DeferReason,
    ) -> Result<DispatchOutcome, DispatchStoreError> {
        Ok(match self.store.defer_unsubmitted(claim, reason).await? {
            DeferOutcome::Deferred => DispatchOutcome::Deferred(reason),
            DeferOutcome::Exhausted => DispatchOutcome::Exhausted,
            DeferOutcome::Refused => {
                self.stop();
                DispatchOutcome::LeaseLost
            }
        })
    }

    async fn finish(
        &self,
        claim: MailClaim,
        outcome: MailOutcome,
    ) -> Result<DispatchOutcome, DispatchStoreError> {
        if self.store.finish(claim, outcome).await? {
            Ok(DispatchOutcome::Finished(outcome))
        } else {
            self.stop();
            Ok(DispatchOutcome::LeaseLost)
        }
    }
}
