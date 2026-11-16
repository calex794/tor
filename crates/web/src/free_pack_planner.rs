//! DROP-R4c: opt-in, read-only SINGLE selection and offline PACK job planning.
//! This planner never uploads, publishes, deletes, grants access or changes an
//! active storage generation. Only the R4b3 fenced worker performs S3 writes.
use std::{sync::Arc, time::Duration};

use sha2::{Digest, Sha256};
use tokio::{
    sync::Semaphore,
    task::JoinHandle,
    time::MissedTickBehavior,
};
use tor_books::{
    repositories::{
        free_pack_attach::PgFreePackCatalog,
        free_pack_planner::{PackPlannerBatch, PackPlannerPolicy},
    },
    services::authoring::AuthorError,
};
use tor_storage::{
    domain::provider::StorageError,
    services::{
        free_chapter::{FreeChapterObject, FreeChapterStore},
        free_pack_write::prepare_free_pack,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannerResult {
    Idle,
    Scheduled(i64),
}

pub struct DropPackPlanner<S> {
    catalog: PgFreePackCatalog,
    single: S,
    private_bucket: String,
    policy: PackPlannerPolicy,
    last_book_id: i64,
    cpu_slot: Arc<Semaphore>,
}

impl<S: FreeChapterStore> DropPackPlanner<S> {
    pub fn new(
        catalog: PgFreePackCatalog,
        single: S,
        private_bucket: String,
        policy: PackPlannerPolicy,
    ) -> Result<Self, &'static str> {
        if policy.validate().is_err()
            || !tor_storage::domain::provider::valid_private_bucket(&private_bucket)
        {
            return Err("Invalid DROP planner bucket or policy");
        }
        Ok(Self {
            catalog,
            single,
            private_bucket,
            policy,
            last_book_id: 0,
            cpu_slot: Arc::new(Semaphore::new(1)),
        })
    }

    /// Keyset walk capped at 32 books per tick, never OFFSET or full-library
    /// scans. One successful admission per tick; the next tick continues
    /// after the previous page. Backpressure is bounded by B's one-live-book
    /// uniqueness index, plus the worker's independent fenced lease.
    pub async fn run_one(&mut self) -> Result<PlannerResult, AuthorError> {
        let ids = self.catalog.planner_next_books(self.last_book_id, 32).await?;
        if ids.is_empty() {
            self.last_book_id = 0;
            return Ok(PlannerResult::Idle);
        }
        self.last_book_id = *ids.last().ok_or(AuthorError::Unavailable)?;
        for book in ids {
            match self.plan_one_book(book).await {
                Ok(Some(id)) => return Ok(PlannerResult::Scheduled(id)),
                Ok(None) | Err(AuthorError::Conflict) => (),
                Err(error) => return Err(error),
            }
        }
        Ok(PlannerResult::Idle)
    }

    /// Build exact deterministic target bytes *before* B schedules a job.
    /// A concurrent planner may do duplicate read-only work, but B accepts
    /// at most one active job per book, and identical descriptors replay
    /// idempotently via the existing R4b1 fingerprint.
    pub async fn plan_one_book(&self, book: i64) -> Result<Option<i64>, AuthorError> {
        let Some(batch) = self.catalog
            .planner_book_batch(book, &self.private_bucket, self.policy)
            .await?
        else {
            return Ok(None);
        };
        let key = deterministic_pack_key(&batch);
        let mut bytes = Vec::with_capacity(batch.sources.len());
        for src in &batch.sources {
            let edition = FreeChapterObject::from_metadata(
                src.chapter_id, src.version_id, src.content_sha256, src.size_bytes
            ).map_err(map_storage)?;
            let source = self.single.get_verified(&edition).await.map_err(map_storage)?;
            bytes.push((edition, source));
        }
        let bucket = self.private_bucket.clone();
        // Timeout or cancellation must not bypass CPU backpressure. The slot
        // is kept inside the blocking task until compression really finishes.
        let slot = self.cpu_slot.clone().acquire_owned().await
            .map_err(|_| AuthorError::Unavailable)?;
        let prepared = tokio::task::spawn_blocking(move || {
            let _cpu_slot = slot;
            prepare_free_pack(&bucket, &key, &bytes)
        })
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .map_err(map_storage)?;
        // Immutable B inventory can change while private SINGLEs are read;
        // B's final admission revalidates each exact source and ownership.
        let job = self.catalog.schedule_pack_job(
            batch.actor_user_id, batch.book_id, prepared.receipt()
        ).await?;
        Ok(Some(job))
    }
}

fn map_storage(error: StorageError) -> AuthorError {
    match error {
        StorageError::Unavailable => AuthorError::Unavailable,
        StorageError::Invalid | StorageError::Conflict | StorageError::Corrupt => AuthorError::Conflict,
    }
}

/// Same book and same exact ordered immutable source inventory yield the same
/// key across retries and concurrent planners. Different versions always
/// get a different private object key; no object can be overwritten.
fn deterministic_pack_key(batch: &PackPlannerBatch) -> String {
    let mut hash = Sha256::new();
    hash.update(b"tor-drop-r4c-auto-pack-v1");
    hash.update(batch.book_id.to_be_bytes());
    for source in &batch.sources {
        hash.update(source.chapter_id.to_be_bytes());
        hash.update(source.version_id.to_be_bytes());
        hash.update(source.content_sha256);
        hash.update(source.size_bytes.to_be_bytes());
    }
    let digest = hash.finalize();
    let fingerprint: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("private/free-packs/auto/{}/{fingerprint}.zst", batch.book_id)
}

/// Deliberately disabled at startup unless a second, explicit operator flag
/// is set. One outstanding S3/CPU planning attempt per process; no HTTP.
pub fn start_background<S: FreeChapterStore + 'static>(
    mut planner: DropPackPlanner<S>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match tokio::time::timeout(Duration::from_secs(300), planner.run_one()).await {
                Ok(Ok(PlannerResult::Idle)) => (),
                Ok(Ok(PlannerResult::Scheduled(_))) => (),
                Ok(Err(error)) => tracing::warn!(?error, "DROP planner admission unavailable"),
                Err(_) => tracing::warn!("DROP planner source/read/build timeout"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_books::repositories::free_pack_sources::ApprovedPackSource;

    fn sample(book_id: i64, version_id: i64) -> PackPlannerBatch {
        PackPlannerBatch {
            book_id,
            actor_user_id: 7,
            sources: vec![ApprovedPackSource {
                chapter_id: 8,
                version_id,
                content_sha256: [5; 32],
                size_bytes: 128,
                bucket: "private-ci-store".to_owned(),
            }],
        }
    }

    #[test]
    fn source_identifiers_and_version_changes_produce_distinct_keys() {
        let k = deterministic_pack_key(&sample(2, 3));
        assert_eq!(k, deterministic_pack_key(&sample(2, 3)));
        assert_ne!(k, deterministic_pack_key(&sample(2, 4)));
        assert_ne!(k, deterministic_pack_key(&sample(3, 3)));
        assert!(k.starts_with("private/free-packs/auto/2/"));
        assert!(k.ends_with(".zst"));
    }
}
