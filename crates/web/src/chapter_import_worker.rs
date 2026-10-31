//! S023: one bounded worker for owner-approved, canonical UTF-8 chapter batches.
use std::{sync::Arc, time::Duration};
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tor_books::{
    repositories::{postgres::PgAuthoring, PgCatalog},
    services::{
        authoring::AuthorError,
        chapter_file_import::{fingerprint, parse_text, MAX_IMPORT_BYTES},
        chapter_import_jobs::{ChapterImportObjectStore, ChapterImportStoreError},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChapterImportWorkerResult {
    Idle,
    Completed,
    Retried,
    Failed,
    Stale,
}

pub struct ChapterImportWorker {
    authoring: PgAuthoring,
    storage: Arc<dyn ChapterImportObjectStore>,
}

impl ChapterImportWorker {
    pub fn new(catalog: &PgCatalog, storage: Arc<dyn ChapterImportObjectStore>) -> Self {
        Self {
            authoring: PgAuthoring::new(catalog.pool()),
            storage,
        }
    }

    pub async fn run_one(&self) -> Result<ChapterImportWorkerResult, &'static str> {
        let Some(claim) = self
            .authoring
            .claim_chapter_import()
            .await
            .map_err(|_| "claim_unavailable")?
        else {
            return Ok(ChapterImportWorkerResult::Idle);
        };

        let expected_hash: [u8; 32] = match claim.source_sha256.as_slice().try_into() {
            Ok(value) => value,
            Err(_) => {
                return self
                    .terminal_failure(&claim, "invalid_source_hash")
                    .await;
            }
        };
        let bytes = match self
            .storage
            .get_verified(
                &claim.source_key,
                claim.source_bytes,
                &expected_hash,
                MAX_IMPORT_BYTES,
            )
            .await
        {
            Ok(bytes) => bytes,
            Err(ChapterImportStoreError::Unavailable) => {
                return self
                    .retryable_failure(&claim, "source_store_unavailable")
                    .await;
            }
            Err(_) => return self.terminal_failure(&claim, "source_object_invalid").await,
        };
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                return self
                    .terminal_failure(&claim, "source_encoding_invalid")
                    .await;
            }
        };
        let chapters = match parse_text(&claim.source_filename, &text) {
            Ok(chapters) => chapters,
            Err(_) => {
                return self
                    .terminal_failure(&claim, "source_structure_invalid")
                    .await;
            }
        };
        if chapters.len() != claim.expected_chapter_count as usize
            || fingerprint(claim.book_id, &chapters).as_slice()
                != claim.approved_fingerprint.as_slice()
        {
            return self.terminal_failure(&claim, "preview_changed").await;
        }

        match self
            .authoring
            .complete_chapter_import_job(&claim, &chapters)
            .await
        {
            Ok(_) => {
                // The succeeded state and all chapters are already committed.
                // Cleanup failure leaves only an unreferenced private staging object.
                let _ = self.storage.delete(&claim.source_key).await;
                Ok(ChapterImportWorkerResult::Completed)
            }
            Err(AuthorError::Unavailable) => {
                self.retryable_failure(&claim, "database_unavailable")
                    .await
            }
            Err(AuthorError::Conflict) => {
                // A current epoch with a chapter-number conflict becomes failed.
                // A cancelled, expired or reclaimed epoch cannot update or delete
                // the source object belonging to the winning worker.
                self.terminal_failure(&claim, "import_conflict").await
            }
            Err(_) => self.terminal_failure(&claim, "book_not_importable").await,
        }
    }

    async fn retryable_failure(
        &self,
        claim: &tor_books::repositories::postgres::chapter_import_jobs::ChapterImportClaim,
        code: &str,
    ) -> Result<ChapterImportWorkerResult, &'static str> {
        self.record_failure(claim, claim.attempts < 8, code).await
    }

    async fn terminal_failure(
        &self,
        claim: &tor_books::repositories::postgres::chapter_import_jobs::ChapterImportClaim,
        code: &str,
    ) -> Result<ChapterImportWorkerResult, &'static str> {
        self.record_failure(claim, false, code).await
    }

    async fn record_failure(
        &self,
        claim: &tor_books::repositories::postgres::chapter_import_jobs::ChapterImportClaim,
        retry: bool,
        code: &str,
    ) -> Result<ChapterImportWorkerResult, &'static str> {
        let changed = self
            .authoring
            .fail_chapter_import(claim.job_id, claim.epoch, retry, code)
            .await
            .map_err(|_| "job_update_unavailable")?;
        if !changed {
            // This worker no longer owns the lease. In particular it must not
            // delete a source that a replacement worker may currently be reading.
            return Ok(ChapterImportWorkerResult::Stale);
        }
        if !retry {
            let _ = self.storage.delete(&claim.source_key).await;
            Ok(ChapterImportWorkerResult::Failed)
        } else {
            Ok(ChapterImportWorkerResult::Retried)
        }
    }
}

/// Explicit opt-in only; a demo, missing PG, or missing private S3 fails closed.
pub fn worker_opt_in(
    setting: Option<&str>,
    demo: bool,
    postgres_configured: bool,
    authenticated: bool,
    private_storage: bool,
) -> Result<bool, &'static str> {
    match setting {
        None | Some("0") => Ok(false),
        Some("1") if !demo && postgres_configured && authenticated && private_storage => Ok(true),
        Some("1") => Err("Chapter import worker requires authenticated PostgreSQL and private S3"),
        Some(_) => Err("TOR_CHAPTER_IMPORT_WORKER_ENABLED must be 0 or 1"),
    }
}

pub fn start_chapter_import_worker_background(
    catalog: PgCatalog,
    storage: Arc<dyn ChapterImportObjectStore>,
) -> JoinHandle<()> {
    let worker = ChapterImportWorker::new(&catalog, storage);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(3));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            // A timed-out task releases its database lease by expiry. Stop this
            // local runner rather than immediately claiming more work.
            match tokio::time::timeout(Duration::from_secs(90), worker.run_one()).await {
                Ok(Ok(
                    ChapterImportWorkerResult::Idle
                    | ChapterImportWorkerResult::Completed
                    | ChapterImportWorkerResult::Retried
                    | ChapterImportWorkerResult::Failed
                    | ChapterImportWorkerResult::Stale,
                )) => {}
                Ok(Err(_)) => {}
                Err(_) => {
                    // The transaction may have committed or rolled back before
                    // timeout cancellation. The job row is the source of truth;
                    // wait briefly, then let its lease determine safe recovery.
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_is_default_off_and_requires_all_capabilities() {
        assert!(!worker_opt_in(None, false, true, true, true).unwrap());
        assert!(!worker_opt_in(Some("0"), false, true, true, true).unwrap());
        assert!(worker_opt_in(Some("1"), false, true, true, true).unwrap());
        for values in [
            (true, true, true, true),
            (false, false, true, true),
            (false, true, false, true),
            (false, true, true, false),
        ] {
            assert!(worker_opt_in(Some("1"), values.0, values.1, values.2, values.3).is_err());
        }
        for value in ["yes", "true", "", "2", "01"] {
            assert!(worker_opt_in(Some(value), false, true, true, true).is_err());
        }
    }
}
