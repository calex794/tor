//! G-09 / S3-RECON-01: read-only, fenced PostgreSQL -> private S3 HEAD audit.
//! Only G-owned checkpoints and sanitized findings are written. Never run GC,
//! change a chapter, revoke a purchase or infer missing objects from S3 outages.
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{EncryptedObjectKind, ObjectId, ObjectRef};

use crate::{
    domain::{object_keys::PrivateObjectKey, provider::valid_private_bucket},
    repositories::reconciliation::InventoryError,
    services::reconciliation::{PrivateRegisteredHead, PrivateScope, RegisteredHeadResult},
};

const PAGE_LIMIT: i64 = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredAuditProgress {
    pub cycle: i64,
    pub expected: i64,
    pub scanned: i64,
    pub has_ref: i64,
    pub healthy: i64,
    pub missing: i64,
    pub referenced_missing: i64,
    pub mismatch: i64,
    pub recently_pending: i64,
    pub unknown_attempts: i64,
    pub retries: i64,
    pub exhausted: bool,
    pub completed: bool,
}

#[derive(Clone)]
pub struct PgRegisteredAudit {
    pool: PgPool,
}

impl PgRegisteredAudit {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Sanitized operator counters, never object ids, paths, URLs or tokens.
    pub async fn progress(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<Option<RegisteredAuditProgress>, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let row: Option<(
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            bool,
            bool,
        )> = sqlx::query_as(
            "SELECT cycle,expected,scanned,has_ref,healthy,missing,
                        referenced_missing,mismatch,recently_pending,
                        unknown_attempts,retries,exhausted,completed
                   FROM g09_registered_object_scans
                  WHERE bucket=$1 AND scope=$2",
        )
        .bind(bucket)
        .bind(scope.kind())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        Ok(row.map(|v| RegisteredAuditProgress {
            cycle: v.0,
            expected: v.1,
            scanned: v.2,
            has_ref: v.3,
            healthy: v.4,
            missing: v.5,
            referenced_missing: v.6,
            mismatch: v.7,
            recently_pending: v.8,
            unknown_attempts: v.9,
            retries: v.10,
            exhausted: v.11,
            completed: v.12,
        }))
    }

    /// One bounded batch per call. The cursor moves only in a PG transaction
    /// after every required HEAD has returned an authoritative result.
    pub async fn scan_page(
        &self,
        store: &impl PrivateRegisteredHead,
        scope: PrivateScope,
    ) -> Result<Option<RegisteredAuditProgress>, InventoryError> {
        let bucket = store.private_bucket();
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        sqlx::query(
            "INSERT INTO g09_registered_object_scans(bucket,scope,expected)
             SELECT $1,$2,COUNT(*)::bigint FROM storage_objects
              WHERE bucket=$1 AND object_kind=$2
             ON CONFLICT (bucket,scope) DO NOTHING",
        )
        .bind(bucket)
        .bind(scope.kind())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        let claim: Option<(i64, Option<String>, i64)> = sqlx::query_as(
            "UPDATE g09_registered_object_scans
                SET lease_epoch=lease_epoch+1,
                    leased_until=clock_timestamp()+interval '180 seconds'
              WHERE bucket=$1 AND scope=$2 AND NOT completed AND NOT exhausted
                AND (leased_until IS NULL OR leased_until<=clock_timestamp())
              RETURNING lease_epoch,cursor_id::text,cycle",
        )
        .bind(bucket)
        .bind(scope.kind())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        let Some((epoch, cursor, cycle)) = claim else {
            return Ok(None);
        };

        let rows = sqlx::query(
            "SELECT s.id::text AS object_id,s.size_bytes,s.sha256_ciphertext,s.state,
                    s.created_at > clock_timestamp()-interval '24 hours' AS fresh,
                    (EXISTS(SELECT 1 FROM chapter_versions v WHERE v.storage_object_id=s.id)
                     OR EXISTS(SELECT 1 FROM chapter_encrypted_release_events e
                               WHERE e.storage_object_id=s.id)
                     OR EXISTS(SELECT 1 FROM chapter_version_assets a
                               WHERE a.storage_object_id=s.id)) AS has_ref
               FROM storage_objects s
              WHERE s.bucket=$1 AND s.object_kind=$2
                AND ($3::uuid IS NULL OR s.id>$3::uuid)
              ORDER BY s.id LIMIT 128",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(cursor.as_deref())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;

        let mut findings = Vec::with_capacity(rows.len());
        let mut last_id = cursor;
        for row in &rows {
            let id: String = row
                .try_get("object_id")
                .map_err(|_| InventoryError::Unavailable)?;
            let bytes: i64 = row
                .try_get("size_bytes")
                .map_err(|_| InventoryError::Unavailable)?;
            let digest: Vec<u8> = row
                .try_get("sha256_ciphertext")
                .map_err(|_| InventoryError::Unavailable)?;
            let state: String = row
                .try_get("state")
                .map_err(|_| InventoryError::Unavailable)?;
            let fresh: bool = row
                .try_get("fresh")
                .map_err(|_| InventoryError::Unavailable)?;
            let has_ref: bool = row
                .try_get("has_ref")
                .map_err(|_| InventoryError::Unavailable)?;
            let kind = match scope {
                PrivateScope::Chapter => EncryptedObjectKind::Chapter,
                PrivateScope::ChapterAsset => EncryptedObjectKind::ChapterAsset,
            };
            let hash: [u8; 32] = digest
                .as_slice()
                .try_into()
                .map_err(|_| InventoryError::Invalid)?;
            let object_id = ObjectId::parse(&id).map_err(|_| InventoryError::Invalid)?;
            let key = PrivateObjectKey::for_object(kind, &object_id);
            let object = ObjectRef::new(
                object_id,
                kind,
                hash,
                u64::try_from(bytes).map_err(|_| InventoryError::Invalid)?,
            )
            .map_err(|_| InventoryError::Invalid)?;
            let result = match store.head_registered(&object).await {
                Ok(status) => status,
                Err(_) => {
                    self.note_unavailable(bucket, scope, epoch).await;
                    return Err(InventoryError::Unavailable);
                }
            };
            let status = match result {
                RegisteredHeadResult::Present => "healthy",
                RegisteredHeadResult::MetadataMismatch => "size_or_sha_mismatch",
                // A GC-fenced deleting row is not evidence of accidental loss.
                RegisteredHeadResult::Missing if state == "deleting" => "recently_pending",
                RegisteredHeadResult::Missing if state == "pending" && fresh => "recently_pending",
                RegisteredHeadResult::Missing if has_ref || state == "attached" => {
                    "referenced_but_missing"
                }
                RegisteredHeadResult::Missing => "unreferenced_missing",
            };
            findings.push((
                Sha256::digest(key.as_str().as_bytes()).to_vec(),
                status,
                state,
                has_ref,
                bytes,
            ));
            last_id = Some(id);
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        let owns: Option<i64> = sqlx::query_scalar(
            "SELECT lease_epoch FROM g09_registered_object_scans
              WHERE bucket=$1 AND scope=$2 AND cycle=$3 AND lease_epoch=$4
                AND leased_until>clock_timestamp() AND NOT completed
              FOR UPDATE",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(cycle)
        .bind(epoch)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        if owns.is_none() {
            return Err(InventoryError::Busy);
        }
        let mut counts = [0_i64; 7];
        for (hash, status, state, has_ref, size) in findings {
            let inserted = sqlx::query(
                "INSERT INTO g09_registered_object_findings
                    (bucket,scope,cycle,key_sha256,status,object_state,has_ref,size_bytes)
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8)
                 ON CONFLICT DO NOTHING",
            )
            .bind(bucket)
            .bind(scope.kind())
            .bind(cycle)
            .bind(&hash)
            .bind(status)
            .bind(&state)
            .bind(has_ref)
            .bind(size)
            .execute(&mut *tx)
            .await
            .map_err(|_| InventoryError::Unavailable)?
            .rows_affected();
            if inserted == 1 {
                counts[0] += 1;
                counts[1] += if has_ref { 1 } else { 0 };
                match status {
                    "healthy" => counts[2] += 1,
                    "referenced_but_missing" => {
                        counts[3] += 1;
                        counts[4] += 1;
                    }
                    "unreferenced_missing" => counts[3] += 1,
                    "size_or_sha_mismatch" => counts[5] += 1,
                    _ => counts[6] += 1,
                }
            }
        }
        sqlx::query(
            "UPDATE g09_registered_object_scans
                SET cursor_id=$5::uuid,scanned=scanned+$6,has_ref=has_ref+$7,
                    healthy=healthy+$8,missing=missing+$9,
                    referenced_missing=referenced_missing+$10,mismatch=mismatch+$11,
                    recently_pending=recently_pending+$12,
                    completed=$13,leased_until=NULL,consecutive_failures=0,
                    checkpoint_at=clock_timestamp()
              WHERE bucket=$1 AND scope=$2 AND cycle=$3 AND lease_epoch=$4",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(cycle)
        .bind(epoch)
        .bind(last_id.as_deref())
        .bind(counts[0])
        .bind(counts[1])
        .bind(counts[2])
        .bind(counts[3])
        .bind(counts[4])
        .bind(counts[5])
        .bind(counts[6])
        .bind(rows.len() < PAGE_LIMIT as usize)
        .execute(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        tx.commit().await.map_err(|_| InventoryError::Unavailable)?;
        self.progress(bucket, scope).await
    }

    /// An outage is an unknown attempt, never a missing object. Keep the cursor
    /// unchanged; after eight consecutive failures, require operator action.
    async fn note_unavailable(&self, bucket: &str, scope: PrivateScope, epoch: i64) {
        let _ = sqlx::query(
            "UPDATE g09_registered_object_scans
                SET leased_until=NULL,retries=retries+1,
                    unknown_attempts=unknown_attempts+1,
                    consecutive_failures=consecutive_failures+1,
                    exhausted=(consecutive_failures+1>=8)
              WHERE bucket=$1 AND scope=$2 AND lease_epoch=$3
                AND leased_until>clock_timestamp()",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(epoch)
        .execute(&self.pool)
        .await;
    }

    /// An operator may explicitly retry an exhausted scan without losing the
    /// cursor or prior evidence. There is no automatic unbounded retry.
    pub async fn resume_exhausted(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<bool, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let updated = sqlx::query(
            "UPDATE g09_registered_object_scans
                SET exhausted=FALSE,consecutive_failures=0,leased_until=NULL
              WHERE bucket=$1 AND scope=$2 AND exhausted AND NOT completed
                AND (leased_until IS NULL OR leased_until<=clock_timestamp())",
        )
        .bind(bucket)
        .bind(scope.kind())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        Ok(updated.rows_affected() == 1)
    }

    /// Explicit next cycle; never started by a failed scan or by a web request.
    /// Previous cycle findings are retained for operator investigation.
    pub async fn begin_next_cycle(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<bool, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let updated = sqlx::query(
            "UPDATE g09_registered_object_scans s
                SET cycle=cycle+1,cursor_id=NULL,scanned=0,has_ref=0,
                    healthy=0,missing=0,referenced_missing=0,mismatch=0,
                    recently_pending=0,unknown_attempts=0,retries=0,
                    consecutive_failures=0,exhausted=FALSE,completed=FALSE,
                    checkpoint_at=NULL,
                    expected=(SELECT COUNT(*) FROM storage_objects
                               WHERE bucket=$1 AND object_kind=$2)
              WHERE s.bucket=$1 AND s.scope=$2 AND s.completed
                AND (leased_until IS NULL OR leased_until<=clock_timestamp())",
        )
        .bind(bucket)
        .bind(scope.kind())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        Ok(updated.rows_affected() == 1)
    }
}
