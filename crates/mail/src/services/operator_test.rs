//! Bounded one-shot diagnostic sender. Not wired to runtime or HTTP here.
use crate::{
    domain::{
        launch::OperatorTestGrant,
        postbox_profile::ProfileMasterKey,
        quota::identity::QuotaKeySigner,
        MailOutcome,
    },
    repositories::launch::PgMailLaunch,
    services::{
        launch::LaunchFence,
        postbox::{PostboxCredentials, PostboxTransport},
    },
};
use std::{sync::Arc, time::Duration};
use tokio::time::timeout;

const TEST_LIMIT: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorTestResult {
    /// No message was submitted by this call; a charge may remain.
    NotSent,
    /// Persisted result. Accepted means SMTP acceptance, not inbox delivery.
    Terminal(MailOutcome),
    /// Lost/ambiguous observation; never retry the attempt.
    Unavailable,
}

pub struct OperatorTestSender {
    launch: PgMailLaunch,
    fence: Arc<LaunchFence>,
    master: Arc<ProfileMasterKey>,
    signer: Arc<QuotaKeySigner>,
    allowlisted_recipient: String,
}

/// Release only this local test approval on cancellation/panic/timeout.
struct RetireTest<'a> {
    fence: &'a LaunchFence,
    grant: &'a OperatorTestGrant,
}

impl Drop for RetireTest<'_> {
    fn drop(&mut self) {
        self.fence.abandon_operator_test(self.grant);
    }
}

impl OperatorTestSender {
    /// The mailbox must come from independently verified server configuration.
    /// Constructing this service does not connect, arm or send.
    pub fn new(
        launch: PgMailLaunch,
        fence: Arc<LaunchFence>,
        master: Arc<ProfileMasterKey>,
        signer: Arc<QuotaKeySigner>,
        allowlisted_recipient: String,
    ) -> Self {
        Self {
            launch,
            fence,
            master,
            signer,
            allowlisted_recipient,
        }
    }

    /// One call, no automatic retry. Share serialization with normal dispatch.
    pub async fn run(&self, grant: OperatorTestGrant) -> OperatorTestResult {
        let _retire = RetireTest {
            fence: &self.fence,
            grant: &grant,
        };
        let Some(_serial) = self.fence.try_dispatch_worker() else {
            return OperatorTestResult::NotSent;
        };
        match timeout(TEST_LIMIT, self.run_inner(&grant)).await {
            Ok(result) => result,
            Err(_) => OperatorTestResult::Unavailable,
        }
    }

    async fn run_inner(&self, grant: &OperatorTestGrant) -> OperatorTestResult {
        use OperatorTestResult::*;
        let Ok(Some(before)) = self.launch.snapshot().await else {
            return Unavailable;
        };
        if self.fence.precheck_operator_test(grant, &before).is_err() {
            return NotSent;
        }
        let Ok(context) = self
            .launch
            .operator_test_context(grant, &self.allowlisted_recipient)
            .await
        else {
            return NotSent;
        };
        let Ok(transport) = PostboxTransport::new(context.profile().identity().sender()) else {
            return NotSent;
        };
        let Ok(secrets) = self.master.open(context.profile()) else {
            return NotSent;
        };
        let Ok(credentials) =
            PostboxCredentials::new(secrets.access().to_owned(), secrets.secret().to_owned())
        else {
            return NotSent;
        };
        drop(secrets);
        if self.fence.precheck_operator_test(grant, &before).is_err() {
            return NotSent;
        }
        // AUTH only, never submits or charges an attempt.
        let Ok(connection) = transport.connect(&credentials).await else {
            return NotSent;
        };
        drop(credentials);
        if self.fence.precheck_operator_test(grant, &before).is_err() {
            return NotSent;
        }
        let Ok(reservation) = self
            .launch
            .reserve_operator_test(grant, &self.allowlisted_recipient, &self.signer)
            .await
        else {
            // Database commit may be ambiguous. No repeat, no refund.
            return Unavailable;
        };
        let Ok(start) = self
            .launch
            .prepare_operator_test(grant, &reservation, &self.allowlisted_recipient, &self.signer)
            .await
        else {
            return Unavailable;
        };
        let Ok(message) = transport.prepare_operator_test(start.recipient(), start.marker()) else {
            return NotSent;
        };
        let Ok(completion) = self.fence.consume_operator_test_start(start) else {
            return NotSent;
        };
        // No await or other operation between consuming permission and submit.
        let submission = connection.submit(message).await;
        let outcome = submission.terminal_outcome();
        match self
            .launch
            .finish_operator_test(&completion, outcome, submission.accepted_id())
            .await
        {
            Ok(true) => Terminal(outcome),
            _ => Unavailable,
        }
    }
}
