//! AUTH-READ-02c2a: bounded B event producer and exact G claim fence.
//! This is an internal queue contract, never a public reading capability.
use super::{GuestPrefixTarget, PgGuestPrefix};
use crate::services::authoring::AuthorError;
use serde::Deserialize;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use tor_contracts::guest_prefix::GUEST_PREFIX_ALGORITHM;

pub const GUEST_PREFIX_EVENT: &str = "GuestPrefixPrepare";
pub const GUEST_PREFIX_CONSUMER: &str = "guest_prefix_prepare_v1";

#[derive(Debug, Eq, PartialEq)]
pub struct GuestPrefixQueuedPage {
    pub enqueued: u64,
    pub next_after_chapter_no: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    book_id: i64,
    chapter_id: i64,
    version_id: i64,
    policy_revision: i64,
    algorithm_version: String,
    source_sha256: Option<String>,
}

/// Constructed only from a real, currently leased, exact event in PG.
/// Contains metadata, never source text or a storage credential.
#[derive(Debug)]
pub struct GuestPrefixJob {
    event_id: String,
    epoch: i64,
    target: GuestPrefixTarget,
    payload: Value,
}

impl GuestPrefixJob {
    pub fn target(&self) -> GuestPrefixTarget {
        self.target
    }
}

fn dedupe_key(target: GuestPrefixTarget) -> String {
    format!(
        "guest-prefix:{}:{}:{}:{}:{}",
        target.book_id,
        target.chapter_id,
        target.version_id,
        target.policy_revision,
        GUEST_PREFIX_ALGORITHM
    )
}

fn valid_event_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, ch)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                ch == b'-'
            } else {
                ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()
            }
        })
}

impl PgGuestPrefix {
    /// One bounded advisory sweep page, followed by one bounded insert.
    /// This does not read a body, call S3, claim an event or activate a mode.
    /// Recheck live eligibility before enqueue; the worker checks again.
    pub async fn enqueue_missing_page(
        &self,
        book_id: i64,
        policy_revision: i64,
        after_chapter_no: i32,
        limit: i64,
    ) -> Result<GuestPrefixQueuedPage, AuthorError> {
        let page = self
            .missing_page(book_id, policy_revision, after_chapter_no, limit)
            .await?;
        let chapters: Vec<i64> = page.targets.iter().map(|target| target.chapter_id).collect();
        let versions: Vec<i64> = page.targets.iter().map(|target| target.version_id).collect();
        let enqueued = if chapters.is_empty() {
            0
        } else {
            sqlx::query(
                "INSERT INTO outbox_events
                   (event_type,schema_version,aggregate_kind,aggregate_id,
                    dedupe_key,payload,required_consumers)
                 SELECT $6,1,'chapter',c.id,
                    'guest-prefix:' || b.id::text || ':' || c.id::text || ':' ||
                    v.id::text || ':' || b.guest_policy_revision::text || ':' || $5::text,
                    jsonb_build_object(
                        'book_id',b.id,'chapter_id',c.id,'version_id',v.id,
                        'policy_revision',b.guest_policy_revision,
                        'algorithm_version',$5::text,
                        'source_sha256',encode(v.content_sha256,'hex')),
                    ARRAY[$7::text]
                 FROM unnest($3::bigint[],$4::bigint[]) AS t(chapter_id,version_id)
                 JOIN chapters c ON c.id=t.chapter_id
                 JOIN books b ON b.id=c.book_id
                 JOIN chapter_publications p ON p.chapter_id=c.id AND p.version_id=t.version_id
                 JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
                 LEFT JOIN chapter_version_guest_prefixes g
                   ON g.chapter_id=c.id AND g.version_id=v.id AND g.algorithm_version=$5
                 WHERE b.id=$1 AND b.guest_policy_revision=$2 AND b.guest_policy_revision>0
                   AND b.visibility='public' AND b.visibility_status='active'
                   AND b.access_level='basic' AND NOT b.admin_hold AND NOT b.is_deleted
                   AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
                   AND c.chapter_no>b.guest_open_through_chapter_no
                   AND c.visibility='public' AND c.access_level='free' AND c.price_amount=0
                   AND NOT c.admin_hold AND NOT c.is_deleted
                   AND (c.reader_released_at IS NULL
                        OR c.reader_released_at<=clock_timestamp())
                   AND g.version_id IS NULL
                 ON CONFLICT(dedupe_key) DO NOTHING",
            )
            .bind(book_id)
            .bind(policy_revision)
            .bind(&chapters)
            .bind(&versions)
            .bind(GUEST_PREFIX_ALGORITHM)
            .bind(GUEST_PREFIX_EVENT)
            .bind(GUEST_PREFIX_CONSUMER)
            .execute(&self.pool)
            .await
            .map_err(|_| AuthorError::Unavailable)?
            .rows_affected()
        };
        Ok(GuestPrefixQueuedPage {
            enqueued,
            next_after_chapter_no: page.next_after_chapter_no,
        })
    }

    /// Resolve a claim returned by the existing G PgOutbox. This is not
    /// sufficient source authority: the next read must check live B facts.
    pub async fn claimed_job(
        &self,
        event_id: &str,
        epoch: i64,
    ) -> Result<Option<GuestPrefixJob>, AuthorError> {
        if !valid_event_id(event_id) || epoch <= 0 {
            return Err(AuthorError::Invalid);
        }
        let row: Option<(i64, String, Value)> = sqlx::query_as(
            "SELECT e.aggregate_id,e.dedupe_key,e.payload FROM outbox_events e
             WHERE e.id=$1::uuid AND e.lease_epoch=$2 AND e.lease_until>clock_timestamp()
               AND e.event_type=$3 AND e.schema_version=1 AND e.aggregate_kind='chapter'
               AND e.required_consumers=ARRAY[$4::text]
               AND e.delivered_at IS NULL AND e.dead_at IS NULL
               AND NOT EXISTS(SELECT 1 FROM event_deliveries d
                              WHERE d.event_id=e.id AND d.consumer=$4)",
        )
        .bind(event_id)
        .bind(epoch)
        .bind(GUEST_PREFIX_EVENT)
        .bind(GUEST_PREFIX_CONSUMER)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some((aggregate_id, key, payload)) = row else {
            return Ok(None);
        };
        // Require the nullable SHA field to be present, not silently omitted.
        if !payload
            .as_object()
            .is_some_and(|fields| fields.len() == 6 && fields.contains_key("source_sha256"))
        {
            return Err(AuthorError::Invalid);
        }
        let decoded: Payload =
            serde_json::from_value(payload.clone()).map_err(|_| AuthorError::Invalid)?;
        let target = GuestPrefixTarget {
            book_id: decoded.book_id,
            chapter_id: decoded.chapter_id,
            version_id: decoded.version_id,
            policy_revision: decoded.policy_revision,
        };
        let valid_hash = decoded.source_sha256.as_ref().is_none_or(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        });
        if target.book_id <= 0
            || target.chapter_id <= 0
            || target.version_id <= 0
            || target.policy_revision <= 0
            || target.chapter_id != aggregate_id
            || decoded.algorithm_version != GUEST_PREFIX_ALGORITHM
            || !valid_hash
            || key != dedupe_key(target)
        {
            return Err(AuthorError::Invalid);
        }
        Ok(Some(GuestPrefixJob {
            event_id: event_id.to_owned(),
            epoch,
            target,
            payload,
        }))
    }
}

/// Called after the book and exact publication locks, and again just before
/// COMMIT. While held, another G claim/retry/ack cannot change this epoch.
pub(super) async fn lock_live_claim(
    tx: &mut Transaction<'_, Postgres>,
    job: &GuestPrefixJob,
    source_hash: &[u8; 32],
) -> Result<(), AuthorError> {
    let current: Option<String> = sqlx::query_scalar(
        "SELECT e.id::text FROM outbox_events e
         WHERE e.id=$1::uuid AND e.lease_epoch=$2 AND e.payload=$3
           AND e.event_type=$4 AND e.required_consumers=ARRAY[$5::text]
           AND e.schema_version=1 AND e.aggregate_kind='chapter' AND e.aggregate_id=$6
           AND e.dedupe_key=$7 AND e.lease_until>clock_timestamp()
           AND e.delivered_at IS NULL AND e.dead_at IS NULL
           AND (e.payload->>'source_sha256' IS NULL
                OR e.payload->>'source_sha256'=encode($8::bytea,'hex'))
           AND NOT EXISTS(SELECT 1 FROM event_deliveries d
                          WHERE d.event_id=e.id AND d.consumer=$5)
         FOR UPDATE",
    )
    .bind(&job.event_id)
    .bind(job.epoch)
    .bind(&job.payload)
    .bind(GUEST_PREFIX_EVENT)
    .bind(GUEST_PREFIX_CONSUMER)
    .bind(job.target.chapter_id)
    .bind(dedupe_key(job.target))
    .bind(source_hash.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if current.is_none() {
        return Err(AuthorError::Conflict);
    }
    Ok(())
}
