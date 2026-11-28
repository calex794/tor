//! One bounded G consumer attempt. No scheduler or public HTTP route.
use super::{prepare_legacy_job, GuestPrefixJob, GuestPrefixPreparation, PgGuestPrefix};
use tor_books::{
    repositories::{
        guest_prefix::{GUEST_PREFIX_CONSUMER, GUEST_PREFIX_EVENT},
        PgCatalog,
    },
    services::authoring::AuthorError,
};
use tor_ingest::repositories::outbox::{ClaimedEvent, FailureDisposition, OutboxError, PgOutbox};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestPrefixWorkerOutcome {
    Idle,
    Prepared(GuestPrefixPreparation),
    Superseded,
    RetryScheduled,
    DeadLetter,
    Stale,
}

enum Admission {
    Finished(GuestPrefixWorkerOutcome),
    Ready(ClaimedEvent, GuestPrefixJob),
}

/// Both ports use the same catalog pool. A worker cannot select an unrelated
/// provider or another outbox consumer from an event payload.
#[derive(Clone)]
pub struct GuestPrefixConsumer {
    repo: PgGuestPrefix,
    outbox: PgOutbox,
}

impl GuestPrefixConsumer {
    pub fn new(catalog: &PgCatalog) -> Self {
        Self {
            repo: PgGuestPrefix::new(catalog.pool().clone()),
            outbox: PgOutbox::new(catalog.pool().clone()),
        }
    }

    pub async fn run_one_legacy(&self) -> Result<GuestPrefixWorkerOutcome, OutboxError> {
        match self.admit().await? {
            Admission::Finished(outcome) => Ok(outcome),
            Admission::Ready(claim, job) => {
                let result = prepare_legacy_job(&self.repo, &job).await;
                self.settle(&claim, &job, result).await
            }
        }
    }

    #[cfg(feature = "s3-free")]
    pub async fn run_one_verified<S: tor_storage::services::free_chapter::FreeChapterStore>(
        &self,
        store: &S,
        trusted_bucket: &str,
    ) -> Result<GuestPrefixWorkerOutcome, OutboxError> {
        match self.admit().await? {
            Admission::Finished(outcome) => Ok(outcome),
            Admission::Ready(claim, job) => {
                let result =
                    super::prepare_verified_job(&self.repo, &job, store, trusted_bucket).await;
                self.settle(&claim, &job, result).await
            }
        }
    }

    async fn admit(&self) -> Result<Admission, OutboxError> {
        let Some(claim) = self
            .outbox
            .claim_next_for_consumer(GUEST_PREFIX_EVENT, GUEST_PREFIX_CONSUMER)
            .await?
        else {
            return Ok(Admission::Finished(GuestPrefixWorkerOutcome::Idle));
        };
        // This event family has exactly one consumer. Do not acknowledge
        // a malformed envelope or attempt to handle another consumer.
        if claim.event_type != GUEST_PREFIX_EVENT
            || claim.required_consumers.len() != 1
            || claim.required_consumers[0] != GUEST_PREFIX_CONSUMER
        {
            return self.failure(&claim, false).await.map(Admission::Finished);
        }
        match self.repo.claimed_job(&claim.id, claim.lease_epoch).await {
            Ok(Some(job)) => Ok(Admission::Ready(claim, job)),
            Ok(None) => self.failure(&claim, true).await.map(Admission::Finished),
            Err(error) => self
                .failure(&claim, error != AuthorError::Invalid)
                .await
                .map(Admission::Finished),
        }
    }

    async fn settle(
        &self,
        claim: &ClaimedEvent,
        job: &GuestPrefixJob,
        result: Result<Option<GuestPrefixPreparation>, AuthorError>,
    ) -> Result<GuestPrefixWorkerOutcome, OutboxError> {
        match result {
            Ok(Some(prepared)) => {
                self.ack(claim, GuestPrefixWorkerOutcome::Prepared(prepared))
                    .await
            }
            Ok(None) | Err(AuthorError::Conflict) => {
                // Ineligibility might be temporary. Only a strictly newer
                // revision is stable supersession of this revision-bound job.
                if self.repo.claimed_policy_superseded(job).await == Ok(true) {
                    self.ack(claim, GuestPrefixWorkerOutcome::Superseded).await
                } else {
                    self.failure(claim, true).await
                }
            }
            Err(_) => self.failure(claim, true).await,
        }
    }

    async fn ack(
        &self,
        claim: &ClaimedEvent,
        outcome: GuestPrefixWorkerOutcome,
    ) -> Result<GuestPrefixWorkerOutcome, OutboxError> {
        match self
            .outbox
            .acknowledge(&claim.id, claim.lease_epoch, GUEST_PREFIX_CONSUMER)
            .await
        {
            Ok(true) => Ok(outcome),
            Ok(false) => Err(OutboxError::Invalid),
            Err(OutboxError::LostLease) => Ok(GuestPrefixWorkerOutcome::Stale),
            Err(error) => Err(error),
        }
    }

    async fn failure(
        &self,
        claim: &ClaimedEvent,
        retryable: bool,
    ) -> Result<GuestPrefixWorkerOutcome, OutboxError> {
        match self
            .outbox
            .record_failure(&claim.id, claim.lease_epoch, retryable)
            .await
        {
            Ok(FailureDisposition::RetryScheduled) => Ok(GuestPrefixWorkerOutcome::RetryScheduled),
            Ok(FailureDisposition::DeadLetter) => Ok(GuestPrefixWorkerOutcome::DeadLetter),
            Err(OutboxError::LostLease) => Ok(GuestPrefixWorkerOutcome::Stale),
            Err(error) => Err(error),
        }
    }
}
