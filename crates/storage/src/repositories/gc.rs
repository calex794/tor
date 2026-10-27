//! G-09/S3-12: durable, lease-fenced orphan registry.
//! Selection happens in one PostgreSQL statement with SKIP LOCKED.
//! A claimed object is moved from ready/pending to deleting before S3 I/O.
//! B attachment requires ready under an UPDATE row lock: deletion wins or
//! attachment wins, never both.
use async_trait::async_trait;
use sqlx::{PgPool, Row};

use crate::{
    domain::{gc::GcClaim, provider::StorageError},
    services::gc::{GcError, OrphanGcRegistry},
};

#[derive(Clone)]
pub struct PgOrphanGc {
    pool: PgPool,
}

impl PgOrphanGc {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Low-cardinality, secret-free counts; an exhausted job needs operator
    /// action and is never automatically deleted after the eighth failure.
    pub async fn counts(&self) -> Result<(i64, i64, i64), GcError> {
        sqlx::query_as(
            "SELECT COUNT(*) FILTER (
                        WHERE state IN ('pending','ready')
                          AND delete_after <= now()
                    )::bigint,
                    COUNT(*) FILTER (
                        WHERE state='deleting'
                          AND gc_attempts < 8
                    )::bigint,
                    COUNT(*) FILTER (
                        WHERE state='deleting'
                          AND gc_attempts >= 8
                    )::bigint
               FROM storage_objects
              WHERE delete_after IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|_| GcError::Unavailable)
    }
}

#[async_trait]
impl OrphanGcRegistry for PgOrphanGc {
    async fn claim_due(&self) -> Result<Option<GcClaim>, GcError> {
        let row = sqlx::query(
            "WITH selected AS (
                SELECT s.id FROM storage_objects AS s
                WHERE s.delete_after <= now()
                  AND s.gc_attempts < 8
                  AND (
                    s.state IN ('pending','ready')
                    OR (
                        s.state='deleting'
                        AND (s.gc_locked_until IS NULL
                             OR s.gc_locked_until <= now())
                    )
                  )
                  AND (s.gc_retry_after IS NULL OR s.gc_retry_after <= now())
                  AND NOT EXISTS (
                      SELECT 1 FROM chapter_versions AS v
                      WHERE v.storage_object_id=s.id
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM chapter_encrypted_release_events AS e
                      WHERE e.storage_object_id=s.id
                  )
                ORDER BY s.delete_after,s.id
                FOR UPDATE OF s SKIP LOCKED LIMIT 1
            )
            UPDATE storage_objects AS s
               SET state='deleting',gc_attempts=s.gc_attempts+1,
                   gc_lease_epoch=s.gc_lease_epoch+1,
                   gc_locked_until=now()+INTERVAL '120 seconds',
                   gc_retry_after=NULL,gc_error_code=NULL
              FROM selected WHERE s.id=selected.id
             RETURNING s.id::text AS object_id,
                       s.object_kind,s.object_key,s.bucket,
                       s.gc_lease_epoch,s.gc_attempts",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| GcError::Unavailable)?;
        row.map(|row| {
            let id: String = row.try_get("object_id").map_err(|_| GcError::Unavailable)?;
            let kind: String = row.try_get("object_kind").map_err(|_| GcError::Unavailable)?;
            let key: String = row.try_get("object_key").map_err(|_| GcError::Unavailable)?;
            let bucket: String = row.try_get("bucket").map_err(|_| GcError::Unavailable)?;
            let epoch: i64 = row.try_get("gc_lease_epoch").map_err(|_| GcError::Unavailable)?;
            let attempts: i32 = row.try_get("gc_attempts").map_err(|_| GcError::Unavailable)?;
            GcClaim::from_database(&id, &kind, &key, bucket, epoch, attempts)
                .map_err(|_: StorageError| GcError::Invalid)
        })
        .transpose()
    }

    async fn finish(&self, claim: &GcClaim) -> Result<(), GcError> {
        // A stale worker cannot finish another worker's lease. The B FK and
        // delete trigger independently veto any historical attachment.
        let removed: Option<String> = sqlx::query_scalar(
            "DELETE FROM storage_objects AS s
              WHERE s.id=$1::uuid AND s.state='deleting'
                AND s.gc_lease_epoch=$2
                AND s.gc_locked_until>now()
                AND NOT EXISTS (
                    SELECT 1 FROM chapter_versions v
                    WHERE v.storage_object_id=s.id
                )
                AND NOT EXISTS (
                    SELECT 1 FROM chapter_encrypted_release_events e
                    WHERE e.storage_object_id=s.id
                )
             RETURNING s.id::text",
        )
        .bind(claim.object_id().as_str())
        .bind(claim.epoch())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| GcError::Unavailable)?;
        removed.ok_or(GcError::LostLease)?;
        Ok(())
    }

    async fn retry(&self, claim: &GcClaim, code: &'static str) -> Result<(), GcError> {
        if !matches!(code, "unavailable" | "corrupt" | "conflict" | "invalid") {
            return Err(GcError::Invalid);
        }
        let recorded: Option<String> = sqlx::query_scalar(
            "UPDATE storage_objects
                SET gc_locked_until=NULL,
                    gc_retry_after=now()+make_interval(
                        secs => LEAST(3600, 30*(1 << (gc_attempts-1)))
                    ),
                    gc_error_code=$3
              WHERE id=$1::uuid AND state='deleting'
                AND gc_lease_epoch=$2 AND gc_locked_until>now()
             RETURNING id::text",
        )
        .bind(claim.object_id().as_str())
        .bind(claim.epoch())
        .bind(code)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| GcError::Unavailable)?;
        recorded.ok_or(GcError::LostLease)?;
        Ok(())
    }
}
