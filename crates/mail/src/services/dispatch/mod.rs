//! One outbound dispatcher. Product handlers resolve trusted source events;
//! they never send email or reserve provider capacity themselves.
use crate::{
    domain::{
        dispatch::{DeferDecision, DeferReason},
        postbox_profile::ProfileMasterKey,
        quota::QuotaKeys,
        MailClaim, MailOutcome,
    },
    repositories::{
        dispatch::DispatchStores, launch::PgMailLaunch, postbox_feedback::PgPostboxFeedback,
        quota::PgMailQuota, PgMailQueue,
    },
    services::{
        launch::{LaunchFence, LaunchScope},
        postbox::{ConnectFailure, PostboxCredentials, PostboxTransport, ResetOrigin},
    },
};
use sha2::{Digest, Sha256};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::{sync::watch, time::timeout};
use tor_contracts::mail::MailPurpose;
use zeroize::Zeroizing;

mod policy;
use policy::{quota_action, QuotaAction};

const STEP_LIMIT: Duration = Duration::from_secs(90);
const TICK: Duration = Duration::from_secs(1);

/// Contains a private transport address, never logged, Debug or serialized.
pub struct ResolvedReset {
    recipient: String,
    keys: QuotaKeys,
}

impl ResolvedReset {
    pub fn new(recipient: String, keys: QuotaKeys) -> Self {
        Self { recipient, keys }
    }

    fn same_source(&self, other: &Self) -> bool {
        self.recipient == other.recipient
            && self.keys.recipient() == other.keys.recipient()
            && self.keys.registration_ip() == other.keys.registration_ip()
            && self.keys.key_binding() == other.keys.key_binding()
    }
}

pub enum SourceDecision {
    PasswordReset(ResolvedReset),
    Cancel,
    Defer(DeferReason),
    Unavailable,
}

pub type Resolve<'a> = Pin<Box<dyn Future<Output = SourceDecision> + Send + 'a>>;

/// Trusted account/service port. Never implemented from HTTP input or a
/// queue-stored address. The same source is resolved again after SMTP AUTH.
pub trait DispatchSource: Send + Sync {
    fn resolve(&self, claim: MailClaim) -> Resolve<'_>;
    fn fresh_reset_proof(&self) -> Option<Zeroizing<String>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerStep {
    Stopped,
    Busy,
    Idle,
    Deferred,
    Terminal(MailOutcome),
    Unavailable,
}

/// Construct explicitly with the shared launch authority and reviewed
/// configuration. Constructing it performs no queries, DNS or SMTP.
pub struct DispatchWorker {
    fence: Arc<LaunchFence>,
    source: Arc<dyn DispatchSource>,
    master: Arc<ProfileMasterKey>,
    origin: ResetOrigin,
    queue: PgMailQueue,
    launch: PgMailLaunch,
    quota: PgMailQuota,
    feedback: PgPostboxFeedback,
}

impl DispatchWorker {
    pub fn new(
        stores: DispatchStores,
        fence: Arc<LaunchFence>,
        source: Arc<dyn DispatchSource>,
        master: Arc<ProfileMasterKey>,
        origin: ResetOrigin,
    ) -> Self {
        Self {
            fence,
            source,
            master,
            origin,
            queue: stores.queue,
            launch: stores.launch,
            quota: stores.quota,
            feedback: stores.feedback,
        }
    }

    /// Explicit runtime task entry point. The caller must share this fence
    /// with the future operator activation/stop handlers. Shutdown drains
    /// the current bounded attempt, not an arbitrary number of queued jobs.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        let mut timer = tokio::time::interval(TICK);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                biased;
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = timer.tick() => {
                    let _ = self.step().await;
                }
            }
        }
        let _ = self.fence.retire();
    }

    /// One bounded, serialized attempt. An error/timeout can mean a committed
    /// reservation or a delivered message, so stop rather than replay.
    pub async fn step(&self) -> WorkerStep {
        let Some(_serial) = self.fence.try_dispatch_worker() else {
            return WorkerStep::Busy;
        };
        if !self.fence.dispatch_armed() {
            return WorkerStep::Stopped;
        }
        match timeout(STEP_LIMIT, self.step_inner()).await {
            Ok(Ok(result)) => result,
            _ => {
                let _ = self.fence.stop();
                WorkerStep::Unavailable
            }
        }
    }

    async fn step_inner(&self) -> Result<WorkerStep, ()> {
        let Some(snapshot) = self.launch.snapshot().await.map_err(|_| ())? else {
            self.fence.stop().map_err(|_| ())?;
            return Ok(WorkerStep::Stopped);
        };
        if self
            .fence
            .permit_reservation(&snapshot, LaunchScope::Dispatch)
            .is_err()
        {
            return Ok(WorkerStep::Stopped);
        }
        self.queue.recover_expired().await.map_err(|_| ())?;
        // Never take a registration/support/news claim before that flow has
        // its trusted source and final consent checks implemented.
        let Some(claim) = self
            .queue
            .claim_next_for(&[MailPurpose::PasswordReset])
            .await
            .map_err(|_| ())?
        else {
            return Ok(WorkerStep::Idle);
        };
        let first = match self.source.resolve(claim).await {
            SourceDecision::PasswordReset(value) => value,
            decision => return self.source_denied(claim, decision).await,
        };
        let Ok(grant) = self.fence.authorize_dispatch(&snapshot, claim) else {
            return self.defer(claim, DeferReason::ActivationUnavailable).await;
        };
        let Some(profile) = self.launch.dispatch_profile(&grant).await.map_err(|_| ())? else {
            self.fence.stop().map_err(|_| ())?;
            return self.defer(claim, DeferReason::ActivationUnavailable).await;
        };
        let identity = profile.identity().identity_id();
        let transport = PostboxTransport::new(profile.identity().sender()).map_err(|_| ())?;
        let secrets = self.master.open(&profile).map_err(|_| ())?;
        let credentials =
            PostboxCredentials::new(secrets.access().to_owned(), secrets.secret().to_owned())
                .map_err(|_| ())?;
        drop(secrets);
        // AUTH can be slow, but never submits a message or charges mail quota.
        if !self.fence.dispatch_armed() {
            return self.defer(claim, DeferReason::ActivationUnavailable).await;
        }
        let connection = match transport.connect(&credentials).await {
            Ok(connection) => connection,
            Err(ConnectFailure::Unavailable) => {
                return self.defer(claim, DeferReason::TransportUnavailable).await;
            }
            Err(ConnectFailure::AuthenticationRejected) => {
                self.fence.stop().map_err(|_| ())?;
                return self.defer(claim, DeferReason::ActivationUnavailable).await;
            }
        };
        drop(credentials);
        let current = match self.source.resolve(claim).await {
            SourceDecision::PasswordReset(value) => value,
            decision => return self.source_denied(claim, decision).await,
        };
        if !first.same_source(&current) {
            return self.finish(claim, MailOutcome::Cancelled).await;
        }
        if !self.fence.dispatch_armed() {
            return self.defer(claim, DeferReason::ActivationUnavailable).await;
        }
        let reservation = self
            .quota
            .reserve_dispatch(&grant, current.keys)
            .await
            .map_err(|_| ())?;
        match quota_action(reservation) {
            QuotaAction::Proceed => {}
            QuotaAction::Defer(reason) => return self.defer(claim, reason).await,
            QuotaAction::Cancel => return self.finish(claim, MailOutcome::Cancelled).await,
            QuotaAction::Stop => {
                self.fence.stop().map_err(|_| ())?;
                return self.defer(claim, DeferReason::ActivationUnavailable).await;
            }
            QuotaAction::Quarantine => return Err(()),
        }

        // From here no retry is legal. A failure before consume_start also
        // leaves a charged counter not consumed by the process approval.
        let Some(token) = self.source.fresh_reset_proof() else {
            self.fence.stop().map_err(|_| ())?;
            return self.finish(claim, MailOutcome::Failed).await;
        };
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let Some(prepared) = self
            .feedback
            .prepare_password_reset_dispatch(&grant, identity, &digest, current.keys)
            .await
            .map_err(|_| ())?
        else {
            self.fence.stop().map_err(|_| ())?;
            return self.finish(claim, MailOutcome::Cancelled).await;
        };
        let message = transport.prepare_reset_message(
            &self.origin,
            &current.recipient,
            token.as_str(),
            prepared.marker(),
        );
        drop(token);
        let Ok(message) = message else {
            return self.abort_prepared(claim, identity, &digest).await;
        };
        let Some(start) = self.launch.dispatch_start(prepared).await.map_err(|_| ())? else {
            return self.abort_prepared(claim, identity, &digest).await;
        };
        if self.fence.consume_dispatch_start(start).is_err() {
            return self.abort_prepared(claim, identity, &digest).await;
        }
        // No intervening await before starting the single-use SMTPS session.
        let submission = connection.submit(message).await;
        let outcome = submission.terminal_outcome();
        if !self
            .feedback
            .finish_password_reset(claim, identity, &digest, outcome, submission.accepted_id())
            .await
            .map_err(|_| ())?
        {
            return Err(());
        }
        if outcome == MailOutcome::Uncertain {
            // The provider may have accepted it. Do not retry; require
            // operator review before starting a new unrelated message.
            self.fence.stop().map_err(|_| ())?;
        }
        Ok(WorkerStep::Terminal(outcome))
    }

    async fn source_denied(
        &self,
        claim: MailClaim,
        decision: SourceDecision,
    ) -> Result<WorkerStep, ()> {
        match decision {
            SourceDecision::Cancel => self.finish(claim, MailOutcome::Cancelled).await,
            SourceDecision::Defer(reason) => self.defer(claim, reason).await,
            SourceDecision::Unavailable | SourceDecision::PasswordReset(_) => Err(()),
        }
    }

    async fn defer(&self, claim: MailClaim, reason: DeferReason) -> Result<WorkerStep, ()> {
        match self
            .queue
            .defer_unreserved(claim, reason)
            .await
            .map_err(|_| ())?
        {
            DeferDecision::Deferred => Ok(WorkerStep::Deferred),
            DeferDecision::Exhausted => Ok(WorkerStep::Terminal(MailOutcome::Failed)),
            DeferDecision::Charged | DeferDecision::Stale => Err(()),
        }
    }

    async fn finish(&self, claim: MailClaim, outcome: MailOutcome) -> Result<WorkerStep, ()> {
        if self.queue.finish(claim, outcome).await.map_err(|_| ())? {
            Ok(WorkerStep::Terminal(outcome))
        } else {
            Err(())
        }
    }

    async fn abort_prepared(
        &self,
        claim: MailClaim,
        identity: &str,
        digest: &[u8; 32],
    ) -> Result<WorkerStep, ()> {
        self.fence.stop().map_err(|_| ())?;
        if self
            .feedback
            .finish_password_reset(claim, identity, digest, MailOutcome::Cancelled, None)
            .await
            .map_err(|_| ())?
        {
            Ok(WorkerStep::Terminal(MailOutcome::Cancelled))
        } else {
            Err(())
        }
    }
}

#[cfg(test)]
mod tests;
