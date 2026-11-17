//! DROP-R4b3: opt-in, lease-fenced offline PACK candidate worker.
//! No HTTP route or live reader cutover. Errors never delete source SINGLEs.
use std::{sync::Arc, time::Duration};

use tokio::{
    sync::Semaphore,
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use tor_books::{
    repositories::{free_pack_attach::PgFreePackCatalog, free_pack_jobs::PackJobClaim},
    services::authoring::AuthorError,
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::free_pack_write_s3::S3FreePackWriter,
    services::{
        free_chapter::{FreeChapterObject, FreeChapterStore},
        free_pack_write::prepare_free_pack,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackWorkerResult {
    Idle,
    Completed(i64),
    Retried,
    Failed,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkerFault {
    Retryable,
    Terminal,
}

impl WorkerFault {
    fn from_storage(error: StorageError) -> Self {
        match error {
            StorageError::Unavailable => Self::Retryable,
            StorageError::Invalid | StorageError::Conflict | StorageError::Corrupt => {
                Self::Terminal
            }
        }
    }

    fn from_author(error: AuthorError) -> Self {
        match error {
            AuthorError::Unavailable => Self::Retryable,
            AuthorError::Invalid
            | AuthorError::Forbidden
            | AuthorError::Missing
            | AuthorError::Conflict => Self::Terminal,
        }
    }
}

pub struct DropPackWorker<S> {
    catalog: PgFreePackCatalog,
    single: S,
    writer: S3FreePackWriter,
    bucket: String,
    lease_seconds: i32,
    cpu_slot: Arc<Semaphore>,
}

impl<S: FreeChapterStore> DropPackWorker<S> {
    pub fn new(
        catalog: PgFreePackCatalog,
        single: S,
        writer: S3FreePackWriter,
        configured_bucket: String,
        lease_seconds: i32,
    ) -> Result<Self, &'static str> {
        if !(15..=900).contains(&lease_seconds)
            || !tor_storage::domain::provider::valid_private_bucket(&configured_bucket)
        {
            return Err("Invalid DROP worker lease or private bucket");
        }
        Ok(Self {
            catalog,
            single,
            writer,
            bucket: configured_bucket,
            lease_seconds,
            cpu_slot: Arc::new(Semaphore::new(1)),
        })
    }

    /// Claims at most one preapproved B job; it never creates specifications
    /// from untrusted manifests and never starts unless explicitly invoked.
    pub async fn run_one(&self) -> Result<PackWorkerResult, AuthorError> {
        let Some(claim) = self.catalog.claim_pack_job(self.lease_seconds).await? else {
            return Ok(PackWorkerResult::Idle);
        };
        let interval = Duration::from_secs((self.lease_seconds as u64 / 3).max(1));
        let mut timer = tokio::time::interval_at(Instant::now() + interval, interval);
        timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let running = self.process_claim(&claim);
        tokio::pin!(running);
        let result = loop {
            tokio::select! {
                ready = &mut running => break ready,
                _ = timer.tick() => {
                    match self.catalog.renew_pack_job(claim.job_id, claim.epoch,
                                                      self.lease_seconds).await {
                        Ok(true) => (),
                        Ok(false) => return Ok(PackWorkerResult::Stale),
                        Err(_) => return Err(AuthorError::Unavailable),
                    }
                },
            }
        };
        let pack_id = match result {
            Ok(pack_id) => return Ok(PackWorkerResult::Completed(pack_id)),
            Err(fault) => fault,
        };
        // Completion may already have committed despite an unknown PG ACK.
        // In that case settle cannot match status='running' and changes nothing.
        let retry = pack_id == WorkerFault::Retryable;
        let changed = self.catalog.settle_pack_job(&claim, retry).await?;
        if !changed {
            return Ok(PackWorkerResult::Stale);
        }
        Ok(if retry && claim.epoch < 8 {
            PackWorkerResult::Retried
        } else {
            PackWorkerResult::Failed
        })
    }

    async fn process_claim(&self, claim: &PackJobClaim) -> Result<i64, WorkerFault> {
        if claim.receipt.target.bucket != self.bucket {
            return Err(WorkerFault::Terminal);
        }
        // A trusted and *current* B SQL lookup establishes where each original
        // SINGLE really lives. A forged job frame cannot redirect G source GET.
        let inventory = self
            .catalog
            .approved_pack_sources(claim)
            .await
            .map_err(WorkerFault::from_author)?;
        let mut source_bytes = Vec::with_capacity(inventory.len());
        for src in inventory {
            if src.bucket != self.bucket {
                return Err(WorkerFault::Terminal);
            }
            let edition = FreeChapterObject::from_metadata(
                src.chapter_id,
                src.version_id,
                src.content_sha256,
                src.size_bytes,
            )
            .map_err(WorkerFault::from_storage)?;
            let bytes = self
                .single
                .get_verified(&edition)
                .await
                .map_err(WorkerFault::from_storage)?;
            source_bytes.push((edition, bytes));
        }
        let bucket = self.bucket.clone();
        let key = claim.receipt.target.pack_key.clone();
        // Pack construction may consume the full 128 MiB decoded budget.
        // A dropped waiter may leave CPU work finishing but never S3/PG effects.
        // A cancelled waiter does not stop spawn_blocking. Hold the CPU slot
        // in the blocking closure itself, so a timeout cannot launch a second
        // 128 MiB pack build before the orphaned task has finished.
        let cpu_slot = self
            .cpu_slot
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| WorkerFault::Retryable)?;
        let prepared = tokio::task::spawn_blocking(move || {
            let _cpu_slot = cpu_slot;
            prepare_free_pack(&bucket, &key, &source_bytes)
        })
        .await
        .map_err(|_| WorkerFault::Retryable)?
        .map_err(WorkerFault::from_storage)?;
        if *prepared.receipt() != claim.receipt {
            return Err(WorkerFault::Terminal);
        }
        // Prevent a known-expired or stolen worker from starting *new* S3 PUTs.
        // Any takeover in flight is still fenced by B's finish_pack_job.
        match self
            .catalog
            .renew_pack_job(claim.job_id, claim.epoch, self.lease_seconds)
            .await
        {
            Ok(true) => (),
            Ok(false) => return Err(WorkerFault::Terminal),
            Err(_) => return Err(WorkerFault::Retryable),
        }
        self.catalog
            .reserve_pack_destination(&claim.receipt.target)
            .await
            .map_err(WorkerFault::from_author)?;
        let verified = self
            .writer
            .upload_and_verify(&prepared)
            .await
            .map_err(WorkerFault::from_storage)?;
        if verified.placement_receipt() != claim.receipt {
            return Err(WorkerFault::Terminal);
        }
        self.catalog
            .finish_pack_job(claim.job_id, claim.epoch, &verified.placement_receipt())
            .await
            .map_err(WorkerFault::from_author)
    }
}

/// Distinct explicit operator switch, disabled by default. It requires a real
/// authenticated PG runtime and private S3; no application flags can bypass it.
pub fn worker_opt_in(
    setting: Option<&str>,
    demo: bool,
    postgres: bool,
    auth: bool,
    private_s3: bool,
) -> Result<bool, &'static str> {
    match setting {
        None | Some("0") => Ok(false),
        Some("1") if !demo && postgres && auth && private_s3 => Ok(true),
        Some("1") => Err("DROP PACK worker requires authenticated PG and private FREE S3"),
        Some(_) => Err("TOR_DROP_PACK_WORKER_ENABLED must be 0 or 1"),
    }
}

/// One job at a time per process; no shared PG lease is held over external I/O.
pub fn start_background<S: FreeChapterStore + 'static>(
    worker: DropPackWorker<S>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            // Cancelling an in-flight operation may leave an orphan PACK, but
            // never changes the active reader: only B's fenced commit can attach.
            match tokio::time::timeout(Duration::from_secs(300), worker.run_one()).await {
                Ok(Ok(PackWorkerResult::Idle | PackWorkerResult::Completed(_))) => (),
                Ok(Ok(other)) => tracing::warn!(?other, "DROP PACK worker attempt not completed"),
                Ok(Err(error)) => tracing::warn!(?error, "DROP PACK worker PG unavailable"),
                Err(_) => tracing::warn!("DROP PACK worker attempt timed out"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_drop_worker_opt_in_fails_closed_without_auth_pg_and_s3() {
        assert_eq!(worker_opt_in(None, false, true, true, true), Ok(false));
        assert_eq!(worker_opt_in(Some("0"), false, true, true, true), Ok(false));
        assert!(worker_opt_in(Some("1"), true, true, true, true).is_err());
        assert!(worker_opt_in(Some("1"), false, false, true, true).is_err());
        assert!(worker_opt_in(Some("1"), false, true, false, true).is_err());
        assert!(worker_opt_in(Some("1"), false, true, true, false).is_err());
        assert!(worker_opt_in(Some("yes"), false, true, true, true).is_err());
        assert_eq!(worker_opt_in(Some("1"), false, true, true, true), Ok(true));
    }
}
