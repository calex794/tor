//! B-owned preparation of a bounded exact-edition derivative.
//! Not an HTTP/API command or a source reader. G supplies verified source
//! bytes; no storage I/O or full body allocation occurs under the book lock.
use crate::services::authoring::AuthorError;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use tor_contracts::guest_prefix::{GuestPrefix, GuestPrefixError, GUEST_PREFIX_ALGORITHM};

mod queue;
mod scheduler;
mod sweep;
pub use queue::{
    GuestPrefixJob, GuestPrefixQueuedPage, GuestPrefixSource, GuestPrefixStoredSource,
    GUEST_PREFIX_CONSUMER, GUEST_PREFIX_EVENT,
};
pub use scheduler::{GuestPrefixSweepCursor, GuestPrefixSweepStep, MAX_GUEST_PREFIX_BOOK_PAGE};
pub use sweep::{GuestPrefixPage, MAX_GUEST_PREFIX_SWEEP_PAGE};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestPrefixTarget {
    pub book_id: i64,
    pub chapter_id: i64,
    pub version_id: i64,
    pub policy_revision: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestPrefixPreparation {
    Stored,
    AlreadyStored,
    NoExcerpt,
}

#[derive(Clone)]
pub struct PgGuestPrefix {
    pool: PgPool,
}

async fn lock_prepared_book(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
    revision: i64,
) -> Result<i32, AuthorError> {
    sqlx::query_scalar(
        "SELECT guest_open_through_chapter_no FROM books
         WHERE id=$1 AND guest_policy_revision=$2
           AND guest_policy_revision>0 AND visibility='public'
           AND visibility_status='active' AND access_level='basic'
           AND NOT admin_hold AND NOT is_deleted
           AND lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
         FOR SHARE",
    )
    .bind(book_id)
    .bind(revision)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?
    .ok_or(AuthorError::Conflict)
}

impl PgGuestPrefix {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Internal worker port. Never mount as an HTTP command or call with
    /// arbitrary browser-provided source. The policy revision is captured
    /// before G reads source; the same revision, cutoff, publication and
    /// exact digest are checked again after I/O. Open books with an explicit
    /// prepared policy revision can be precomputed without enabling the gate.
    pub async fn prepare_current(
        &self,
        target: GuestPrefixTarget,
        verified_source: &str,
    ) -> Result<GuestPrefixPreparation, AuthorError> {
        self.prepare_inner(target, verified_source, None).await
    }

    /// The background consumer must use this entrypoint, not prepare_current.
    /// A valid source and an obsolete claim are never enough to write.
    pub async fn prepare_claimed(
        &self,
        job: &GuestPrefixJob,
        verified_source: &str,
    ) -> Result<GuestPrefixPreparation, AuthorError> {
        self.prepare_inner(job.target(), verified_source, Some(job))
            .await
    }

    async fn prepare_inner(
        &self,
        target: GuestPrefixTarget,
        verified_source: &str,
        job: Option<&GuestPrefixJob>,
    ) -> Result<GuestPrefixPreparation, AuthorError> {
        if target.book_id <= 0
            || target.chapter_id <= 0
            || target.version_id <= 0
            || target.policy_revision <= 0
            || verified_source.len() > 200_000
            || verified_source.contains('\0')
        {
            return Err(AuthorError::Invalid);
        }
        let projection = match GuestPrefix::from_source(verified_source) {
            Ok(prefix) => Some(prefix),
            Err(GuestPrefixError::UnsupportedLiteralMarkup) => None,
            Err(_) => return Err(AuthorError::Invalid),
        };
        let source_hash: [u8; 32] = Sha256::digest(verified_source.as_bytes()).into();

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        // Existing B lock order: book before chapter/publication. No work
        // is committed for stale settings, held/hidden books or later versions.
        let cutoff = lock_prepared_book(&mut tx, target.book_id, target.policy_revision).await?;
        let exact: Option<i64> = sqlx::query_scalar(
            "SELECT v.id FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             WHERE c.book_id=$1 AND c.id=$2 AND p.version_id=$3
               AND c.chapter_no>$4 AND c.access_level='free' AND c.price_amount=0
               AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=clock_timestamp())
               AND (
                 (v.body_markdown IS NULL AND v.content_sha256=$5)
                 OR (v.body_markdown IS NOT NULL
                   AND sha256(convert_to(v.body_markdown,'UTF8'))=$5
                   AND (v.content_sha256 IS NULL OR v.content_sha256=$5))
               )
             FOR SHARE OF c,p",
        )
        .bind(target.book_id)
        .bind(target.chapter_id)
        .bind(target.version_id)
        .bind(cutoff)
        .bind(source_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if exact != Some(target.version_id) {
            return Err(AuthorError::Conflict);
        }

        // Book/chapter/publication locks precede the outbox lock. The
        // successor cannot claim this row while we commit the exact result.
        if let Some(job) = job {
            queue::lock_live_claim(&mut tx, job, &source_hash).await?;
        }

        // Unsupported literal markup is a terminal derivative result, not a
        // missing row. Persist only its source identity and an empty prefix.
        // A ready but tiny chapter may also have an empty prefix.
        let (status, count, prefix) = match &projection {
            Some(projection) => (
                "ready",
                i32::try_from(projection.visible_scalar_count())
                    .map_err(|_| AuthorError::Invalid)?,
                projection.maximum_prefix(),
            ),
            None => ("unsupported", 0, ""),
        };
        let prefix_hash: [u8; 32] = Sha256::digest(prefix.as_bytes()).into();
        let result = sqlx::query(
            "INSERT INTO chapter_version_guest_prefixes
                 (chapter_id,version_id,algorithm_version,source_sha256,
                  visible_scalar_count,prefix_text,prefix_sha256,projection_status)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT(chapter_id,version_id,algorithm_version) DO NOTHING",
        )
        .bind(target.chapter_id)
        .bind(target.version_id)
        .bind(GUEST_PREFIX_ALGORITHM)
        .bind(source_hash.as_slice())
        .bind(count)
        .bind(prefix)
        .bind(prefix_hash.as_slice())
        .bind(status)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let outcome = if result.rows_affected() == 1 {
            GuestPrefixPreparation::Stored
        } else {
            // Idempotent only for byte-identical verified derivation. Never
            // silently replace a corrupted or incompatible existing result.
            let identical: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM chapter_version_guest_prefixes
                 WHERE chapter_id=$1 AND version_id=$2 AND algorithm_version=$3
                   AND source_sha256=$4 AND visible_scalar_count=$5
                   AND prefix_text=$6 AND prefix_sha256=$7 AND projection_status=$8)",
            )
            .bind(target.chapter_id)
            .bind(target.version_id)
            .bind(GUEST_PREFIX_ALGORITHM)
            .bind(source_hash.as_slice())
            .bind(count)
            .bind(prefix)
            .bind(prefix_hash.as_slice())
            .bind(status)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if !identical {
                return Err(AuthorError::Conflict);
            }
            GuestPrefixPreparation::AlreadyStored
        };
        if let Some(job) = job {
            queue::lock_live_claim(&mut tx, job, &source_hash).await?;
        }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        if projection.is_none() {
            Ok(GuestPrefixPreparation::NoExcerpt)
        } else {
            Ok(outcome)
        }
    }
}
