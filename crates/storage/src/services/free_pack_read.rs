//! DROP-R3a: **inert** server-only bounded PACK frame reader.
//!
//! A PACK is never presigned or passed to the browser. C resolves the exact
//! reader-authorized edition twice: once before remote I/O and once before
//! any decoded source leaves this service. This does not route live HTTP.
//!
//! A future publisher must independently verify the whole immutable PACK and
//! its sidecar manifest against the trusted PostgreSQL receipts. The range
//! reader verifies one decoded edition; it cannot recompute a whole-PACK hash
//! from a single frame and does NOT make an unverified PACK safe to publish.

use async_trait::async_trait;
use std::{future::Future, time::Duration};
use tokio::time::{timeout_at, Instant};

use crate::{
    domain::provider::StorageError,
    services::{
        crypto::sha256_bytes,
        free_chapter::MAX_FREE_MARKDOWN_BYTES,
        free_placement::{FreeReadPlan, ValidatedPackReference},
    },
};

#[async_trait]
pub trait PackFrameStore: Send + Sync {
    /// Fetch only the trusted edition frame, never a caller-provided range.
    /// Implementations must enforce exact HTTP Content-Range and body length.
    async fn fetch_frame(&self, frame: &ValidatedPackReference) -> Result<Vec<u8>, StorageError>;
}

const PACK_READ_BUDGET: Duration = Duration::from_secs(20);

/// A single monotonic budget for selecting a PACK, reading it, and
/// rechecking the placement. Start before the first catalog lookup.
pub struct PackReadDeadline(Instant);

impl PackReadDeadline {
    pub fn start() -> Self {
        Self(Instant::now() + PACK_READ_BUDGET)
    }

    pub async fn wait<T>(&self, phase: impl Future<Output = T>) -> Result<T, StorageError> {
        if Instant::now() >= self.0 {
            return Err(StorageError::Unavailable);
        }
        let result = timeout_at(self.0, phase)
            .await
            .map_err(|_| StorageError::Unavailable)?;
        // A ready phase can win polling even after the deadline. Stop before
        // dispatching another S3 request or accepting any late result.
        if Instant::now() >= self.0 {
            return Err(StorageError::Unavailable);
        }
        Ok(result)
    }
}

/// Decode exactly one Zstd frame; bound decompression to B's byte count.
///
/// A valid single-frame stream must consume the entire supplied encoded
/// range. Refuse appended unrelated frames or skippable neighbors. SHA256,
/// canonical UTF-8 and the authoritative edition length are checked after
/// decoding, independently of the seek table's non-cryptographic checksum.
pub fn decode_exact_frame(
    reference: &ValidatedPackReference,
    encoded: &[u8],
) -> Result<Vec<u8>, StorageError> {
    if encoded.len() as u64 != reference.metadata().frame_bytes
        || !(1..=MAX_FREE_MARKDOWN_BYTES).contains(&reference.edition().size_bytes())
    {
        return Err(StorageError::Corrupt);
    }

    let first_frame_size =
        zstd::zstd_safe::find_frame_compressed_size(encoded).map_err(|_| StorageError::Corrupt)?;
    if first_frame_size != encoded.len() {
        return Err(StorageError::Corrupt);
    }

    let max_decoded =
        usize::try_from(reference.edition().size_bytes()).map_err(|_| StorageError::Invalid)?;
    // Decompression output length alone is insufficient: a forged Zstd
    // window header could allocate far more history than a 200 KB chapter
    // needs. Independent frames are encoded from one edition <=200 KB.
    let mut decoder = zstd::bulk::Decompressor::new().map_err(|_| StorageError::Corrupt)?;
    decoder
        .window_log_max(20)
        .map_err(|_| StorageError::Corrupt)?;
    let decoded = decoder
        .decompress(encoded, max_decoded)
        .map_err(|_| StorageError::Corrupt)?;
    if decoded.len() != max_decoded
        || sha256_bytes(&decoded) != *reference.edition().sha256()
        || std::str::from_utf8(&decoded).is_err()
    {
        return Err(StorageError::Corrupt);
    }
    Ok(decoded)
}

/// Internal read primitive. No URL, S3 credentials or whole-pack payload is
/// ever returned. Every lookup must perform the real C authorization and
/// exact free-edition/placement generation lookup in PostgreSQL.
///
/// Recheck after the remote fetch+decompression; a revoked/changed edition
/// or changed placement invalidates the whole attempt and yields no bytes.
/// The caller supplies the live reader authorization and placement lookup.
pub async fn read_reauthorized_pack<Lookup, Fut, Store>(
    mut resolve: Lookup,
    store: &Store,
) -> Result<Vec<u8>, StorageError>
where
    Lookup: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<FreeReadPlan>, StorageError>>,
    Store: PackFrameStore,
{
    let deadline = PackReadDeadline::start();
    let before = deadline
        .wait(resolve())
        .await??
        .ok_or(StorageError::Invalid)?;
    let frame = before.pack_frame().ok_or(StorageError::Invalid)?.clone();
    let encoded = deadline.wait(store.fetch_frame(&frame)).await??;
    let source = decode_exact_frame(&frame, &encoded)?;
    match deadline.wait(resolve()).await?? {
        None => Err(StorageError::Invalid),
        Some(after) if after != before => Err(StorageError::Conflict),
        Some(_) => Ok(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{
        free_chapter::FreeChapterObject,
        free_placement::{plan_after_acl, FreePhysicalPlacement, PackMetadata},
    };
    use std::{
        collections::VecDeque,
        future::{pending, ready},
        sync::Mutex,
    };

    struct FakeFrameStore {
        bytes: Vec<u8>,
        fail: bool,
        calls: Mutex<Vec<String>>,
    }
    impl FakeFrameStore {
        fn new(bytes: Vec<u8>) -> Self {
            Self {
                bytes,
                fail: false,
                calls: Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait]
    impl PackFrameStore for FakeFrameStore {
        async fn fetch_frame(
            &self,
            frame: &ValidatedPackReference,
        ) -> Result<Vec<u8>, StorageError> {
            self.calls.lock().unwrap().push(frame.gateway_range());
            if self.fail {
                return Err(StorageError::Unavailable);
            }
            Ok(self.bytes.clone())
        }
    }

    fn edition(id: i64, text: &str) -> FreeChapterObject {
        FreeChapterObject::for_markdown(id, id + 300, text).unwrap()
    }
    fn frame(
        edition: FreeChapterObject,
        offset: usize,
        bytes: &[u8],
        seek_end: usize,
    ) -> ValidatedPackReference {
        let metadata = PackMetadata {
            bucket: "tor-private-pack-ci".to_owned(),
            key: "private/free-packs/ci/repack-r3a.zst".to_owned(),
            pack_bytes: (seek_end + 33) as u64,
            seek_table_start: seek_end as u64,
            frame_offset: offset as u64,
            frame_bytes: bytes.len() as u64,
            pack_sha256: [23; 32],
            manifest_sha256: [29; 32],
        };
        ValidatedPackReference::new(edition, metadata).unwrap()
    }
    fn plan(reference: ValidatedPackReference) -> FreeReadPlan {
        let edition = reference.edition().clone();
        plan_after_acl(&edition, FreePhysicalPlacement::Pack(reference)).unwrap()
    }

    #[tokio::test]
    async fn exact_neighbor_range_only_yields_authorized_edition_bytes() {
        let original1 = "Соседняя глава №1: секрет\n";
        let original2 = "Разрешённая глава №2: безопасно\n";
        let encoded1 = zstd::bulk::compress(original1.as_bytes(), 3).unwrap();
        let encoded2 = zstd::bulk::compress(original2.as_bytes(), 3).unwrap();
        let offset = encoded1.len();
        let authorized = plan(frame(
            edition(12, original2),
            offset,
            &encoded2,
            offset + encoded2.len(),
        ));
        let store = FakeFrameStore::new(encoded2.clone());
        let mut calls = 0;
        let result = read_reauthorized_pack(
            || {
                calls += 1;
                ready(Ok(Some(authorized.clone())))
            },
            &store,
        )
        .await
        .unwrap();
        assert_eq!(result, original2.as_bytes());
        assert_eq!(calls, 2, "lookup C rights both before and after S3 I/O");
        assert_eq!(
            store.calls.lock().unwrap().as_slice(),
            &[format!("bytes={}-{}", offset, offset + encoded2.len() - 1)]
        );
        assert!(!String::from_utf8_lossy(&result).contains(original1));
    }

    #[tokio::test]
    async fn denied_before_fetch_cannot_read_any_pack_frame() {
        let store = FakeFrameStore::new(Vec::new());
        let result = read_reauthorized_pack(|| ready(Ok(None)), &store).await;
        assert_eq!(result, Err(StorageError::Invalid));
        assert!(store.calls.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_selection_that_stalls_or_finishes_at_the_deadline_never_dispatches_s3() {
        let text = "A valid edition in the old PACK";
        let bytes = zstd::bulk::compress(text.as_bytes(), 3).unwrap();
        let old = plan(frame(edition(53, text), 0, &bytes, bytes.len()));
        let store = FakeFrameStore::new(bytes);

        assert_eq!(
            read_reauthorized_pack(pending, &store).await,
            Err(StorageError::Unavailable)
        );
        assert_eq!(
            read_reauthorized_pack(
                || async {
                    tokio::time::sleep(PACK_READ_BUDGET).await;
                    Ok(Some(old.clone()))
                },
                &store,
            )
            .await,
            Err(StorageError::Unavailable)
        );
        assert!(store.calls.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn selection_and_range_share_one_budget_and_a_pending_range_is_dropped() {
        struct PendingRange(Mutex<Vec<&'static str>>);
        #[async_trait]
        impl PackFrameStore for PendingRange {
            async fn fetch_frame(
                &self,
                _: &ValidatedPackReference,
            ) -> Result<Vec<u8>, StorageError> {
                struct FrameInFlight<'a>(&'a Mutex<Vec<&'static str>>);
                impl Drop for FrameInFlight<'_> {
                    fn drop(&mut self) {
                        self.0.lock().unwrap().push("dropped");
                    }
                }
                self.0.lock().unwrap().push("dispatched");
                let _inflight = FrameInFlight(&self.0);
                pending().await
            }
        }

        let text = "No old bytes after a stalled Range";
        let encoded = zstd::bulk::compress(text.as_bytes(), 3).unwrap();
        let old = plan(frame(edition(54, text), 0, &encoded, encoded.len()));
        let store = PendingRange(Mutex::default());
        let start = Instant::now();
        assert_eq!(
            read_reauthorized_pack(
                || async {
                    tokio::time::sleep(Duration::from_secs(12)).await;
                    Ok(Some(old.clone()))
                },
                &store,
            )
            .await,
            Err(StorageError::Unavailable)
        );
        assert_eq!(start.elapsed(), PACK_READ_BUDGET);
        assert_eq!(*store.0.lock().unwrap(), ["dispatched", "dropped"]);
    }

    #[tokio::test(start_paused = true)]
    async fn late_reauthorization_never_returns_a_verified_frame() {
        let text = "Successfully decoded, but the read window has elapsed";
        let encoded = zstd::bulk::compress(text.as_bytes(), 3).unwrap();
        let selected = plan(frame(edition(55, text), 0, &encoded, encoded.len()));
        let store = FakeFrameStore::new(encoded);
        let mut calls = 0;

        assert_eq!(
            read_reauthorized_pack(
                || {
                    calls += 1;
                    let selected = selected.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(11)).await;
                        Ok(Some(selected))
                    }
                },
                &store,
            )
            .await,
            Err(StorageError::Unavailable)
        );
        assert_eq!(calls, 2);
        assert_eq!(store.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn single_edition_cannot_be_misrouted_to_pack_backend() {
        let own = edition(20, "S349 single text");
        let direct = FreeReadPlan::DirectSingle(own);
        let store = FakeFrameStore::new(Vec::new());
        let result = read_reauthorized_pack(|| ready(Ok(Some(direct.clone()))), &store).await;
        assert_eq!(result, Err(StorageError::Invalid));
        assert!(store.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn admin_hold_or_pg_outage_after_fetch_yields_no_plaintext() {
        let text = "Free V1 pinned to an authorized reader";
        let bytes = zstd::bulk::compress(text.as_bytes(), 3).unwrap();
        let authorized = plan(frame(edition(41, text), 0, &bytes, bytes.len()));
        let store = FakeFrameStore::new(bytes);

        let mut lookup = VecDeque::from([Ok(Some(authorized.clone())), Ok(None)]);
        assert_eq!(
            read_reauthorized_pack(|| ready(lookup.pop_front().unwrap()), &store).await,
            Err(StorageError::Invalid),
        );
        let mut lookup =
            VecDeque::from([Ok(Some(authorized.clone())), Err(StorageError::Unavailable)]);
        assert_eq!(
            read_reauthorized_pack(|| ready(lookup.pop_front().unwrap()), &store).await,
            Err(StorageError::Unavailable),
        );
    }

    #[tokio::test]
    async fn different_version_digest_or_generation_after_s3_is_rejected() {
        let text = "Version 1 exact source";
        let bytes = zstd::bulk::compress(text.as_bytes(), 3).unwrap();
        let authorized = plan(frame(edition(50, text), 0, &bytes, bytes.len()));
        let changed = plan(frame(edition(51, text), 0, &bytes, bytes.len()));
        let store = FakeFrameStore::new(bytes);
        let mut results = VecDeque::from([Ok(Some(authorized)), Ok(Some(changed))]);
        assert_eq!(
            read_reauthorized_pack(|| ready(results.pop_front().unwrap()), &store).await,
            Err(StorageError::Conflict),
        );
    }

    #[tokio::test]
    async fn short_corrupt_appended_frame_wrong_digest_or_oversize_fails_closed() {
        let expected = "Текст А. 😀";
        let encoded = zstd::bulk::compress(expected.as_bytes(), 3).unwrap();
        let correct = frame(edition(64, expected), 0, &encoded, encoded.len());
        let shorter = &encoded[..encoded.len() - 1];
        assert_eq!(
            decode_exact_frame(&correct, shorter),
            Err(StorageError::Corrupt)
        );

        let mut corrupted = encoded.clone();
        let damage_offset = corrupted.len() / 2;
        corrupted[damage_offset] ^= 0xff;
        assert_eq!(
            decode_exact_frame(&correct, &corrupted),
            Err(StorageError::Corrupt)
        );

        let mut appended = encoded.clone();
        appended.extend_from_slice(&encoded);
        let double = frame(edition(64, expected), 0, &appended, appended.len());
        assert_eq!(
            decode_exact_frame(&double, &appended),
            Err(StorageError::Corrupt)
        );

        let forged =
            FreeChapterObject::from_metadata(64, 364, [7; 32], expected.len() as u64).unwrap();
        let wrong = frame(forged, 0, &encoded, encoded.len());
        assert_eq!(
            decode_exact_frame(&wrong, &encoded),
            Err(StorageError::Corrupt)
        );

        let huge = "A".repeat(200_001);
        let oversized = zstd::bulk::compress(huge.as_bytes(), 3).unwrap();
        let capped = frame(edition(64, expected), 0, &oversized, oversized.len());
        assert_eq!(
            decode_exact_frame(&capped, &oversized),
            Err(StorageError::Corrupt)
        );
    }

    #[tokio::test]
    async fn storage_failure_never_decodes_or_releases_bytes() {
        let text = "Only a valid S3 Range can yield source";
        let encoded = zstd::bulk::compress(text.as_bytes(), 3).unwrap();
        let authorized = plan(frame(edition(75, text), 0, &encoded, encoded.len()));
        let mut store = FakeFrameStore::new(encoded);
        store.fail = true;
        let result = read_reauthorized_pack(|| ready(Ok(Some(authorized.clone()))), &store).await;
        assert_eq!(result, Err(StorageError::Unavailable));
    }
}
