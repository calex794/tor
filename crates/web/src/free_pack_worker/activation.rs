//! DROP-05b: bounded, recoverable offline activation worker.
//! An R4a candidate is already physically committed before this job begins.
//! Only B's all-member activation CAS may mark the activation succeeded.
use std::time::Duration;

use tokio::{
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use tor_books::{
    repositories::{
        free_pack_activation::{ActivationDisposition, PackActivationClaim},
        free_pack_attach::PgFreePackCatalog,
    },
    services::authoring::AuthorError,
};
use tor_storage::{
    domain::provider::valid_private_bucket,
    repositories::free_pack_proof_s3::S3PackProofStore,
    services::free_chapter::FreeChapterStore,
};

use super::activate_one_queued_verified_pack;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivationWorkerResult {
    Idle,
    Activated,
    AlreadyActive,
    Retried,
    Failed,
    Stale,
}

pub struct DropPackActivationWorker<S> {
    catalog: PgFreePackCatalog,
    single: S,
    proof: S3PackProofStore,
    bucket: String,
    lease_seconds: i32,
}

impl<S: FreeChapterStore> DropPackActivationWorker<S> {
    pub fn new(
        catalog: PgFreePackCatalog,
        single: S,
        proof: S3PackProofStore,
        bucket: String,
        lease_seconds: i32,
    ) -> Result<Self, &'static str> {
        if !(15..=900).contains(&lease_seconds) || !valid_private_bucket(&bucket) {
            return Err("Invalid DROP activation lease or private bucket");
        }
        Ok(Self {
            catalog,
            single,
            proof,
            bucket,
            lease_seconds,
        })
    }

    /// Process at most one durable R4a success. An interrupted proof has no
    /// B effects; after a lost PG commit ACK, B has already atomically marked
    /// both the active pointers and this activation succeeded.
    pub async fn run_one(&self) -> Result<ActivationWorkerResult, AuthorError> {
        let Some(claim) = self.catalog.claim_pack_activation(self.lease_seconds).await? else {
            return Ok(ActivationWorkerResult::Idle);
        };
        if claim.pack.receipt.target.bucket != self.bucket {
            return self.reject(&claim, AuthorError::Conflict).await;
        }

        let interval = Duration::from_secs((self.lease_seconds as u64 / 3).max(1));
        let mut heartbeat = tokio::time::interval_at(Instant::now() + interval, interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let work = activate_one_queued_verified_pack(
            &self.catalog,
            &self.proof,
            &self.single,
            &claim,
        );
        tokio::pin!(work);
        let outcome = loop {
            tokio::select! {
                done = &mut work => break done,
                _ = heartbeat.tick() => {
                    match self.catalog
                        .renew_pack_activation(&claim, self.lease_seconds).await
                    {
                        Ok(true) => (),
                        Ok(false) => return Ok(ActivationWorkerResult::Stale),
                        Err(_) => return Err(AuthorError::Unavailable),
                    }
                },
            }
        };
        match outcome {
            Ok(tor_books::repositories::free_pack_cutover::PackActivation::Activated) => {
                Ok(ActivationWorkerResult::Activated)
            }
            Ok(tor_books::repositories::free_pack_cutover::PackActivation::AlreadyActive) => {
                Ok(ActivationWorkerResult::AlreadyActive)
            }
            Err(error) => self.reject(&claim, error).await,
        }
    }

    async fn reject(
        &self,
        claim: &PackActivationClaim,
        error: AuthorError,
    ) -> Result<ActivationWorkerResult, AuthorError> {
        // Immutable corrupt/forged source or revoked rights do not loop.
        // Transient provider/PG outages back off and are bounded to 8 tries.
        let retryable = matches!(error, AuthorError::Unavailable);
        let action = if retryable {
            ActivationDisposition::Retryable
        } else {
            ActivationDisposition::Terminal
        };
        if !self.catalog.settle_pack_activation(claim, action).await? {
            return Ok(ActivationWorkerResult::Stale);
        }
        Ok(if retryable && claim.attempts < 8 {
            ActivationWorkerResult::Retried
        } else {
            ActivationWorkerResult::Failed
        })
    }
}

/// Deliberate independent opt-in; the existing PACK candidate worker flag
/// does not enable read-pointer mutation. Never enabled by mere import.
pub fn cutover_opt_in(
    setting: Option<&str>,
    demo: bool,
    postgres: bool,
    auth: bool,
    private_s3: bool,
    pack_reader: bool,
    pack_worker: bool,
) -> Result<bool, &'static str> {
    match setting {
        None | Some("0") => Ok(false),
        Some("1")
            if !demo && postgres && auth && private_s3 && pack_reader && pack_worker =>
        {
            Ok(true)
        }
        Some("1") => Err("DROP cutover requires authenticated PG, private S3, PACK worker and reader"),
        Some(_) => Err("TOR_DROP_PACK_CUTOVER_ENABLED must be 0 or 1"),
    }
}

/// One queued activation at a time per process; PostgreSQL leases fence
/// concurrent processes. Cancelling a proof leaves an expiring durable job.
pub fn start_background<S: FreeChapterStore + 'static>(
    worker: DropPackActivationWorker<S>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match tokio::time::timeout(Duration::from_secs(300), worker.run_one()).await {
                Ok(Ok(ActivationWorkerResult::Idle | ActivationWorkerResult::Activated
                    | ActivationWorkerResult::AlreadyActive)) => (),
                Ok(Ok(other)) => tracing::warn!(?other, "DROP activation attempt not completed"),
                Ok(Err(error)) => tracing::warn!(?error, "DROP activation worker PG unavailable"),
                Err(_) => tracing::warn!("DROP activation proof timed out; lease retry pending"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutover_is_separate_and_fails_closed_without_every_dependency() {
        assert_eq!(cutover_opt_in(None, false, true, true, true, true, true), Ok(false));
        assert_eq!(cutover_opt_in(Some("0"), false, true, true, true, true, true), Ok(false));
        assert_eq!(cutover_opt_in(Some("1"), false, true, true, true, true, true), Ok(true));
        for args in [
            (true, true, true, true, true, true),
            (false, false, true, true, true, true),
            (false, true, false, true, true, true),
            (false, true, true, false, true, true),
            (false, true, true, true, false, true),
            (false, true, true, true, true, false),
        ] {
            assert!(cutover_opt_in(
                Some("1"), args.0, args.1, args.2, args.3, args.4, args.5
            )
            .is_err());
        }
        assert!(cutover_opt_in(Some("yes"), false, true, true, true, true, true).is_err());
    }
}
