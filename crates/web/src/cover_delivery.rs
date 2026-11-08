//! H-web-09: same-origin read of privately stored, verified book-cover WebP.
//! B owns publication and current cover identity; G owns bounded object I/O.
//! This bridge never returns a bucket/key, origin, signed URL or user input.
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
use tor_books::repositories::{
    cover_publications::PgPublicCoverPublications,
    cover_slots::{PgCoverSlots, PublicCoverKind},
};
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

#[derive(Clone)]
pub struct CoverDelivery {
    slots: PgCoverSlots,
    objects: Arc<dyn CoverObjectStore>,
    publications: Option<PgPublicCoverPublications>,
    inflight: Arc<Semaphore>,
}

impl CoverDelivery {
    pub fn new(slots: PgCoverSlots, objects: Arc<dyn CoverObjectStore>) -> Self {
        Self {
            slots,
            objects,
            publications: None,
            // Every request is capped at 8 MiB; bound worst-case resident bytes.
            inflight: Arc::new(Semaphore::new(6)),
        }
    }

    /// Explicit opt-in for privileged CDN publishing. The default construction
    /// must fail closed: an S3 adapter alone cannot publish outside the ledger.
    pub fn with_publication_registry(mut self, registry: PgPublicCoverPublications) -> Self {
        self.publications = Some(registry);
        self
    }

    pub async fn available_card_ids(&self, ids: &[u64]) -> Result<HashSet<u64>, CoverError> {
        self.slots
            .public_card_ids(ids)
            .await
            .map_err(|_| CoverError::Unavailable)
    }

    /// Read-only, opt-in CDN URLs. No publish side effects on page requests.
    /// Only ledger-confirmed current BASIC/public identities may be returned.
    /// Without an explicit registry and public CDN origin this fails closed.
    pub async fn current_cdn_urls(
        &self,
        ids: &[u64],
        kind: PublicCoverKind,
    ) -> Result<HashMap<u64, String>, CoverError> {
        let Some(registry) = self.publications.as_ref() else {
            return Err(CoverError::Forbidden);
        };
        let Some(origin) = self.objects.public_cdn_origin() else {
            return Err(CoverError::Forbidden);
        };
        if ids.len() > 100 {
            return Err(CoverError::Invalid);
        }
        // Public CDN origin is fixed by the operator, never an input URL.
        let host = origin.strip_prefix("https://").ok_or(CoverError::Invalid)?;
        if host.is_empty()
            || host
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-')
            || host.starts_with('-')
            || host.starts_with('.')
            || host.ends_with('.')
        {
            return Err(CoverError::Invalid);
        }
        let book_ids: Vec<i64> = ids
            .iter()
            .map(|id| i64::try_from(*id).map_err(|_| CoverError::Invalid))
            .collect::<Result<_, _>>()?;
        let copies = registry
            .current_cdn_copies(&book_ids, kind)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        copies
            .into_iter()
            .map(|cover| {
                let id = u64::try_from(cover.book_id).map_err(|_| CoverError::Unavailable)?;
                Ok((id, format!("{origin}/{}", cover.object_key())))
            })
            .collect()
    }

    /// A short-lived S3 grant for one currently attached and public WebP.
    /// The B-owned projection is read both before and after signing. A bearer
    /// URL remains usable until expiry even if a book is hidden subsequently.
    pub async fn direct_grant(
        &self,
        book_id: u64,
        kind: PublicCoverKind,
    ) -> Result<Option<(String, [u8; 32], i64)>, CoverError> {
        if self.objects.direct_read_origin().is_none() {
            return Err(CoverError::Forbidden);
        }
        let book_id = i64::try_from(book_id).map_err(|_| CoverError::Invalid)?;
        let object = self
            .slots
            .public_variant(book_id, kind)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let Some(object) = object else {
            return Ok(None);
        };
        if object.size_bytes <= 0 || object.size_bytes > 8 * 1024 * 1024 {
            return Err(CoverError::Invalid);
        }
        let signed = self
            .objects
            .presign_variant_get(&object.storage_key, Duration::from_secs(60))
            .await?;
        if self
            .slots
            .public_variant(book_id, kind)
            .await
            .map_err(|_| CoverError::Unavailable)?
            .as_ref()
            != Some(&object)
        {
            return Ok(None);
        }
        Ok(Some((signed, object.sha256, object.size_bytes)))
    }

    /// Privileged default-off publisher. The B-owned reservation is durable
    /// before S3 COPY, so a crash can be recovered without guessing S3 keys.
    /// CDN purge and recovery workers remain separate and are NOT enabled here.
    pub async fn publish_cdn_variant(
        &self,
        book_id: u64,
        kind: PublicCoverKind,
    ) -> Result<Option<String>, CoverError> {
        if self.objects.public_cdn_origin().is_none() {
            return Err(CoverError::Forbidden);
        }
        let registry = self.publications.as_ref().ok_or(CoverError::Forbidden)?;
        let book_id_i64 = i64::try_from(book_id).map_err(|_| CoverError::Invalid)?;
        // A DB transaction verifies that the Book is public and records the
        // exact copy identity before any side-effect in Object Storage.
        let reserved = registry
            .reserve_current(book_id_i64, kind)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let Some(reserved) = reserved else {
            return Ok(None);
        };
        let variant = match kind {
            PublicCoverKind::Card => "card",
            PublicCoverKind::Main => "main",
        };
        let object = self
            .slots
            .public_variant(book_id_i64, kind)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let Some(object) = object else {
            return Ok(None);
        };
        if object.revision != reserved.slot_revision
            || object.sha256 != reserved.sha256
            || object.size_bytes != reserved.size_bytes
        {
            return Ok(None);
        }
        // Limit the uncommitted publication window; the recovery lease is
        // significantly longer and no S3 operation starts after expiry.
        let url = tokio::time::timeout(
            Duration::from_secs(60),
            self.objects.publish_public_cover(
                book_id,
                reserved.slot_revision,
                variant,
                &object.storage_key,
                &reserved.sha256,
                reserved.size_bytes,
            ),
        )
        .await
        .map_err(|_| CoverError::Unavailable)??;
        let confirmed = registry.confirm_copy(reserved.id).await;
        match confirmed {
            Ok(true) => Ok(Some(url)),
            Ok(false) => {
                // B already revoked this identity. Even if deletion fails,
                // the durable row is queued for retry after a new worker starts.
                self.objects
                    .remove_public_cover(book_id, reserved.slot_revision, variant, &reserved.sha256)
                    .await?;
                Ok(None)
            }
            Err(_) => {
                // A crash or database outage cannot make the public URL valid:
                // the prior committed reservation remains recoverable.
                let _ = self
                    .objects
                    .remove_public_cover(book_id, reserved.slot_revision, variant, &reserved.sha256)
                    .await;
                Err(CoverError::Unavailable)
            }
        }
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
