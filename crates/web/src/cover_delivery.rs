//! H-web-09: same-origin read of privately stored, verified book-cover WebP.
//! B owns publication and current cover identity; G owns bounded object I/O.
//! This bridge never returns a bucket/key, origin, signed URL or user input.
use sha2::{Digest, Sha256};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tor_books::repositories::cover_slots::{PgCoverSlots, PublicCoverKind};
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

#[derive(Clone)]
pub struct CoverDelivery {
    slots: PgCoverSlots,
    objects: Arc<dyn CoverObjectStore>,
    inflight: Arc<Semaphore>,
}

impl CoverDelivery {
    pub fn new(slots: PgCoverSlots, objects: Arc<dyn CoverObjectStore>) -> Self {
        Self {
            slots,
            objects,
            // Every request is capped at 8 MiB; bound worst-case resident bytes.
            inflight: Arc::new(Semaphore::new(6)),
        }
    }

    pub async fn available_card_ids(&self, ids: &[u64]) -> Result<HashSet<u64>, CoverError> {
        self.slots
            .public_card_ids(ids)
            .await
            .map_err(|_| CoverError::Unavailable)
    }

    /// Return None for invisible, stale or unknown covers. Return Unavailable
    /// for failed private storage and integrity checks, never partial bytes.
    pub async fn read(
        &self,
        book_id: u64,
        kind: PublicCoverKind,
    ) -> Result<Option<Vec<u8>>, CoverError> {
        let Ok(book_id) = i64::try_from(book_id) else {
            return Ok(None);
        };
        if book_id <= 0 {
            return Ok(None);
        }
        // Reject excess concurrent requests, including their S3 allocations.
        let _permit = self
            .inflight
            .clone()
            .try_acquire_owned()
            .map_err(|_| CoverError::Unavailable)?;
        let object = self
            .slots
            .public_variant(book_id, kind)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let Some(object) = object else {
            return Ok(None);
        };
        let limit = usize::try_from(object.size_bytes).map_err(|_| CoverError::Unavailable)?;
        if limit == 0 || limit > 8 * 1024 * 1024 {
            return Err(CoverError::Unavailable);
        }
        let bytes = tokio::time::timeout(
            Duration::from_secs(8),
            self.objects.get_bounded(&object.storage_key, limit),
        )
        .await
        .map_err(|_| CoverError::Unavailable)?
        .map_err(|_| CoverError::Unavailable)?;
        let checksum: [u8; 32] = Sha256::digest(&bytes).into();
        if bytes.len() != limit
            || bytes.get(..4) != Some(&b"RIFF"[..])
            || bytes.get(8..12) != Some(&b"WEBP"[..])
            || checksum != object.sha256
        {
            return Err(CoverError::Unavailable);
        }
        // A moderator, author or second worker may have changed the book
        // while the private object was read. Re-check before sending bytes.
        let current = self
            .slots
            .public_variant(book_id, kind)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        if current.as_ref() != Some(&object) {
            return Ok(None);
        }
        Ok(Some(bytes))
    }
}
