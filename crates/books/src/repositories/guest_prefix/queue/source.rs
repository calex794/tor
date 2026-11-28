//! B-owned exact source lookup for a live guest-prefix preparation claim.
//! Internal metadata/body only: neither an owner credential nor a reader grant.
use super::{dedupe_key, GuestPrefixJob, GUEST_PREFIX_CONSUMER, GUEST_PREFIX_EVENT};
use crate::{
    repositories::{
        free_pack_attach::PgFreePackCatalog, free_pack_reader::VerifiedFreePackCandidate,
        guest_prefix::PgGuestPrefix,
    },
    services::authoring::AuthorError,
};
use sha2::{Digest, Sha256};
use tor_contracts::reader_text::valid_reader_length;

#[derive(Eq, PartialEq)]
pub struct GuestPrefixStoredSource {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub bucket: String,
    /// None is the original S349 SINGLE without an active generation pointer.
    pub generation: Option<i64>,
}

/// Intentionally no Debug implementation: legacy Markdown must not be logged.
#[derive(Eq, PartialEq)]
pub enum GuestPrefixSource {
    Legacy(String),
    Single(GuestPrefixStoredSource),
    Pack {
        edition: GuestPrefixStoredSource,
        frame: VerifiedFreePackCandidate,
    },
}

type SourceRow = (
    Option<String>,
    Option<Vec<u8>>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<i64>,
    bool,
);

impl PgGuestPrefix {
    async fn claimed_source_row(
        &self,
        job: &GuestPrefixJob,
    ) -> Result<Option<SourceRow>, AuthorError> {
        let target = job.target();
        // One exact claim/edition snapshot. Only the legacy row can contain a
        // PG body. S3 lookup and GET happen outside any book or outbox lock.
        sqlx::query_as(
            "SELECT v.body_markdown,v.content_sha256,a.generation,
                    g.placement_kind,g.size_bytes,pack.bucket,s.bucket,s.size_bytes,
                    (e.payload->>'source_sha256' IS NOT DISTINCT FROM
                     encode(v.content_sha256,'hex'))
             FROM outbox_events e
             JOIN chapters c ON c.id=e.aggregate_id
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             LEFT JOIN free_edition_active_storage a
               ON a.chapter_id=c.id AND a.version_id=v.id
             LEFT JOIN free_edition_storage_generations g
               ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
              AND g.generation=a.generation AND g.content_sha256=v.content_sha256
             LEFT JOIN free_storage_packs pack ON pack.id=g.pack_id
             LEFT JOIN free_chapter_storage_objects s
               ON s.chapter_id=c.id AND s.version_id=v.id AND s.sha256=v.content_sha256
              AND EXISTS(SELECT 1 FROM d001_free_single_admission_gates gate
                         WHERE gate.chapter_id=s.chapter_id AND gate.version_id=s.version_id
                           AND gate.sealed_at IS NULL)
             WHERE e.id=$1::uuid AND e.lease_epoch=$2 AND e.payload=$3
               AND e.event_type=$4 AND e.required_consumers=ARRAY[$5::text]
               AND e.schema_version=1 AND e.aggregate_kind='chapter'
               AND e.dedupe_key=$10 AND e.lease_until>clock_timestamp()
               AND e.delivered_at IS NULL AND e.dead_at IS NULL
               AND NOT EXISTS(SELECT 1 FROM event_deliveries d
                              WHERE d.event_id=e.id AND d.consumer=$5)
               AND c.id=$6 AND b.id=$7 AND v.id=$8 AND b.guest_policy_revision=$9
               AND b.guest_policy_revision>0 AND b.visibility='public'
               AND b.visibility_status='active' AND b.access_level='basic'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
               AND c.chapter_no>b.guest_open_through_chapter_no
               AND c.visibility='public' AND c.access_level='free' AND c.price_amount=0
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=clock_timestamp())",
        )
        .bind(&job.event_id)
        .bind(job.epoch)
        .bind(&job.payload)
        .bind(GUEST_PREFIX_EVENT)
        .bind(GUEST_PREFIX_CONSUMER)
        .bind(target.chapter_id)
        .bind(target.book_id)
        .bind(target.version_id)
        .bind(target.policy_revision)
        .bind(dedupe_key(target))
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)
    }

    /// None means no currently eligible live claim. It is not a permanent
    /// obsolete decision or a receipt. A caller must still check the same
    /// claim at write time and distinguish retryable missing source metadata.
    pub async fn claimed_source(
        &self,
        job: &GuestPrefixJob,
    ) -> Result<Option<GuestPrefixSource>, AuthorError> {
        let Some(row) = self.claimed_source_row(job).await? else {
            return Ok(None);
        };
        let (body, hash, generation, kind, gen_size, pack_bucket, single_bucket, single_size, matches) =
            &row;
        if !matches {
            return Err(AuthorError::Conflict);
        }
        let registered: Option<[u8; 32]> = hash
            .as_ref()
            .map(|value| {
                value
                    .as_slice()
                    .try_into()
                    .map_err(|_| AuthorError::Unavailable)
            })
            .transpose()?;
        if let Some(markdown) = body {
            let actual: [u8; 32] = Sha256::digest(markdown.as_bytes()).into();
            if !valid_reader_length(markdown)
                || markdown.len() > 200_000
                || markdown.contains('\0')
                || registered.is_some_and(|expected| actual != expected)
            {
                return Err(AuthorError::Conflict);
            }
            return Ok(Some(GuestPrefixSource::Legacy(markdown.clone())));
        }

        let sha256 = registered.ok_or(AuthorError::Unavailable)?;
        if sha256 == [0; 32] {
            return Err(AuthorError::Unavailable);
        }
        let (packed, size, bucket) = match (*generation, kind.as_deref()) {
            (None, None) => (false, *single_size, single_bucket.as_ref()),
            (Some(current), Some("single")) if current > 0 && gen_size == single_size => {
                (false, *single_size, single_bucket.as_ref())
            }
            (Some(current), Some("pack")) if current > 0 => {
                (true, *gen_size, pack_bucket.as_ref())
            }
            _ => return Err(AuthorError::Unavailable),
        };
        let size_bytes = u64::try_from(size.ok_or(AuthorError::Unavailable)?)
            .map_err(|_| AuthorError::Unavailable)?;
        let bucket = bucket.ok_or(AuthorError::Unavailable)?.clone();
        if !(1..=200_000).contains(&size_bytes) || bucket.is_empty() {
            return Err(AuthorError::Unavailable);
        }
        let edition = GuestPrefixStoredSource {
            sha256,
            size_bytes,
            bucket,
            generation: *generation,
        };
        if !packed {
            return Ok(Some(GuestPrefixSource::Single(edition)));
        }

        // Reuse the active, independently proved PACK catalog. A merely
        // registered candidate or an active but unproved PACK is not usable.
        let target = job.target();
        let frame = PgFreePackCatalog::new(self.pool.clone())
            .active_verified_free_range(
                target.chapter_id,
                target.version_id,
                edition.sha256,
                edition.size_bytes,
                &edition.bucket,
            )
            .await?
            .ok_or(AuthorError::Unavailable)?;
        let Some(fresh) = self.claimed_source_row(job).await? else {
            return Ok(None);
        };
        if fresh != row {
            return Err(AuthorError::Conflict);
        }
        Ok(Some(GuestPrefixSource::Pack { edition, frame }))
    }
}
