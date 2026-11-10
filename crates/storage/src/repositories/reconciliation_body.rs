//! G-09: explicitly triggered, read-only, bounded PG -> S3 GET/SHA-256 audit.
//! No S3 writes, no user entitlements, no raw object keys in findings.
//! One page downloads at most 8 * 4 MiB of private ciphertext.
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use tor_contracts::paid_delivery::{EncryptedObjectKind, ObjectId, ObjectRef};

use crate::{
    domain::{object_keys::PrivateObjectKey, provider::valid_private_bucket},
    repositories::reconciliation::InventoryError,
    services::reconciliation::{
        PrivateRegisteredBodyProbe, PrivateScope, RegisteredBodyResult, G09_BODY_GET_MAX_BYTES,
    },
};

const PAGE_LIMIT: i64 = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodyAuditProgress {
    pub cycle: i64,
    pub expected: i64,
    pub scanned: i64,
    pub verified: i64,
    pub missing: i64,
    pub body_mismatch: i64,
    pub metadata_mismatch: i64,
    pub skipped: i64,
    pub unknown_attempts: i64,
    pub retries: i64,
    pub exhausted: bool,
    pub completed: bool,
}

#[derive(Clone)]
pub struct PgBodyAudit {
    pool: PgPool,
}

impl PgBodyAudit {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Summary contains no UUID, S3 path, cipher text, cursor or token.
    pub async fn progress(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<Option<BodyAuditProgress>, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let row = sqlx::query(
            "SELECT cycle,expected,scanned,verified,missing,body_mismatch,
                    metadata_mismatch,skipped,unknown_attempts,retries,exhausted,completed
               FROM g09_body_sha_scans WHERE bucket=$1 AND scope=$2",
        )
        .bind(bucket)
        .bind(scope.kind())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;

        row.map(|row| -> Result<BodyAuditProgress, InventoryError> {
            let count = |name: &str| {
                row.try_get::<i64, _>(name)
                    .map_err(|_| InventoryError::Unavailable)
            };
            let flag = |name: &str| {
                row.try_get::<bool, _>(name)
                    .map_err(|_| InventoryError::Unavailable)
            };
            Ok(BodyAuditProgress {
                cycle: count("cycle")?,
                expected: count("expected")?,
                scanned: count("scanned")?,
                verified: count("verified")?,
                missing: count("missing")?,
                body_mismatch: count("body_mismatch")?,
                metadata_mismatch: count("metadata_mismatch")?,
                skipped: count("skipped")?,
                unknown_attempts: count("unknown_attempts")?,
                retries: count("retries")?,
                exhausted: flag("exhausted")?,
                completed: flag("completed")?,
            })
        })
        .transpose()
    }

    /// At most eight registered objects per invocation; caller must explicitly
    /// run another page. No skipped row is mislabelled as a corrupted file.
    pub async fn scan_page(
        &self,
        store: &impl PrivateRegisteredBodyProbe,
        scope: PrivateScope,
    ) -> Result<Option<BodyAuditProgress>, InventoryError> {
        let bucket = store.private_bucket();
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        sqlx::query(
            "INSERT INTO g09_body_sha_scans(bucket,scope,expected)
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
            "UPDATE g09_body_sha_scans
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
            "SELECT id::text AS object_id,size_bytes,sha256_ciphertext,state
               FROM storage_objects
              WHERE bucket=$1 AND object_kind=$2
                AND ($3::uuid IS NULL OR id>$3::uuid)
              ORDER BY id LIMIT 8",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(cursor.as_deref())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;

        let complete = rows.len() < PAGE_LIMIT as usize;
        let scanned = rows.len() as i64;
        let mut counts = [0_i64; 5]; // verified, missing, bytes, metadata, skipped
        let mut findings = Vec::new();
        let mut last = cursor;

        for row in &rows {
            let id: String = row.try_get("object_id").map_err(|_| InventoryError::Unavailable)?;
            let size: i64 = row.try_get("size_bytes").map_err(|_| InventoryError::Unavailable)?;
            let hash: Vec<u8> = row.try_get("sha256_ciphertext")
                .map_err(|_| InventoryError::Unavailable)?;
            let state: String = row.try_get("state").map_err(|_| InventoryError::Unavailable)?;
            last = Some(id.clone());

            // Pending/deleting/oversized objects have no evidence of corruption;
            // do not GET or classify them as missing.
            if !matches!(state.as_str(), "ready" | "attached")
                || !(17..=G09_BODY_GET_MAX_BYTES as i64).contains(&size)
                || hash.len() != 32
            {
                counts[4] += 1;
                continue;
            }

            let object_id = ObjectId::parse(&id).map_err(|_| InventoryError::Invalid)?;
            let kind = match scope {
                PrivateScope::Chapter => EncryptedObjectKind::Chapter,
                PrivateScope::ChapterAsset => EncryptedObjectKind::ChapterAsset,
            };
            let digest: [u8; 32] = hash.as_slice().try_into()
                .map_err(|_| InventoryError::Invalid)?;
            let object = ObjectRef::new(object_id.clone(), kind, digest, size as u64)
                .map_err(|_| InventoryError::Invalid)?;
            let result = match store.probe_registered_body(&object).await {
                Ok(result) => result,
                Err(_) => {
                    self.note_unavailable(bucket, scope, epoch).await;
                    return Err(InventoryError::Unavailable);
                }
            };
            let (index, status) = match result {
                RegisteredBodyResult::Verified => (0, None),
                RegisteredBodyResult::Missing => (1, Some("missing")),
                RegisteredBodyResult::BodyMismatch => (2, Some("body_mismatch")),
                RegisteredBodyResult::MetadataMismatch => (3, Some("metadata_mismatch")),
            };
            counts[index] += 1;
            if let Some(status) = status {
                let key = PrivateObjectKey::for_object(kind, &object_id);
                findings.push((
                    Sha256::digest(key.as_str().as_bytes()).to_vec(),
                    status,
                    size,
                ));
            }
        }

        let mut tx = self.pool.begin().await.map_err(|_| InventoryError::Unavailable)?;
        let owned: Option<(i64,)> = sqlx::query_as(
            "SELECT lease_epoch FROM g09_body_sha_scans
              WHERE bucket=$1 AND scope=$2 AND cycle=$3 AND lease_epoch=$4
                AND leased_until>clock_timestamp()
              FOR UPDATE",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(cycle)
        .bind(epoch)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        if owned.is_none() {
            return Err(InventoryError::Busy);
        }

        for (key_hash, status, size) in findings {
            sqlx::query(
                "INSERT INTO g09_body_sha_findings
                     (bucket,scope,cycle,key_sha256,status,size_bytes)
                 VALUES($1,$2,$3,$4,$5,$6)
                 ON CONFLICT (bucket,scope,cycle,key_sha256)
                 DO UPDATE SET status=EXCLUDED.status,
                               size_bytes=EXCLUDED.size_bytes,
                               observed_at=clock_timestamp()",
            )
            .bind(bucket)
            .bind(scope.kind())
            .bind(cycle)
            .bind(key_hash)
            .bind(status)
            .bind(size)
            .execute(&mut *tx)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        }
        let result = sqlx::query(
            "UPDATE g09_body_sha_scans SET
                 cursor_id=$5::uuid,scanned=scanned+$6,
                 verified=verified+$7,missing=missing+$8,
                 body_mismatch=body_mismatch+$9,
                 metadata_mismatch=metadata_mismatch+$10,
                 skipped=skipped+$11,completed=$12,
                 leased_until=NULL,consecutive_failures=0,
                 checkpoint_at=clock_timestamp()
              WHERE bucket=$1 AND scope=$2 AND lease_epoch=$3 AND cycle=$4",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(epoch)
        .bind(cycle)
        .bind(last.as_deref())
        .bind(scanned)
        .bind(counts[0])
        .bind(counts[1])
        .bind(counts[2])
        .bind(counts[3])
        .bind(counts[4])
        .bind(complete)
        .execute(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        if result.rows_affected() != 1 {
            return Err(InventoryError::Busy);
        }
        tx.commit().await.map_err(|_| InventoryError::Unavailable)?;
        self.progress(bucket, scope).await
    }

    /// S3 transport/permission failure does not advance the cursor or mark
    /// a missing file. Stop after eight failed attempts until explicit resume.
    async fn note_unavailable(&self, bucket: &str, scope: PrivateScope, epoch: i64) {
        let _ = sqlx::query(
            "UPDATE g09_body_sha_scans
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

    pub async fn resume_exhausted(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<bool, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let result = sqlx::query(
            "UPDATE g09_body_sha_scans
                SET exhausted=FALSE,consecutive_failures=0,leased_until=NULL
              WHERE bucket=$1 AND scope=$2 AND exhausted AND NOT completed
                AND (leased_until IS NULL OR leased_until<=clock_timestamp())",
        )
        .bind(bucket)
        .bind(scope.kind())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn begin_next_cycle(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<bool, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let result = sqlx::query(
            "UPDATE g09_body_sha_scans
                SET cycle=cycle+1,cursor_id=NULL,scanned=0,verified=0,
                    missing=0,body_mismatch=0,metadata_mismatch=0,skipped=0,
                    unknown_attempts=0,retries=0,consecutive_failures=0,
                    exhausted=FALSE,completed=FALSE,checkpoint_at=NULL,
                    expected=(SELECT COUNT(*) FROM storage_objects
                               WHERE bucket=$1 AND object_kind=$2)
              WHERE bucket=$1 AND scope=$2 AND completed
                AND (leased_until IS NULL OR leased_until<=clock_timestamp())",
        )
        .bind(bucket)
        .bind(scope.kind())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        Ok(result.rows_affected() == 1)
    }
}
