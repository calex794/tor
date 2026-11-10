//! G-09/S3-RECON-01: resumable, non-destructive S3 -> PG inventory.
//! This module only writes its own metadata/checkpoint tables. It never changes
//! storage_objects, chapter_versions, entitlements, or any S3 object.
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::time::{Duration, SystemTime};

use crate::{
    domain::{object_keys::PrivateObjectKey, provider::valid_private_bucket},
    services::reconciliation::{PrivateInventoryLister, PrivateInventoryPage, PrivateScope},
};

const PUT_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryError {
    Invalid,
    Busy,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryProgress {
    pub scanned: i64,
    pub registered: i64,
    pub recently_pending: i64,
    pub unregistered_candidates: i64,
    pub size_mismatch: i64,
    pub unknown: i64,
    pub unknown_attempts: i64,
    pub retries: i64,
    pub exhausted: bool,
    pub completed: bool,
}

/// A scan is explicitly scoped to the configured private bucket and the two
/// canonical encrypted namespaces; cover/CDN and public derivatives are excluded.
#[derive(Clone)]
pub struct PgPrivateInventory {
    pool: PgPool,
}

impl PgPrivateInventory {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A read-only summary; never exposes S3 keys or opaque continuation tokens.
    pub async fn progress(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<Option<InventoryProgress>, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let row = sqlx::query(
            "SELECT scanned,registered,recently_pending,unregistered_candidates,
                    size_mismatch,unknown,unknown_attempts,retries,exhausted,completed
               FROM g09_private_inventory_scans
              WHERE bucket=$1 AND scope=$2",
        )
        .bind(bucket)
        .bind(scope.kind())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        row.map(|value| -> Result<InventoryProgress, InventoryError> {
            let count = |name: &str| {
                value.try_get::<i64, _>(name)
                    .map_err(|_| InventoryError::Unavailable)
            };
            let flag = |name: &str| {
                value.try_get::<bool, _>(name)
                    .map_err(|_| InventoryError::Unavailable)
            };
            Ok(InventoryProgress {
                scanned: count("scanned")?,
                registered: count("registered")?,
                recently_pending: count("recently_pending")?,
                unregistered_candidates: count("unregistered_candidates")?,
                size_mismatch: count("size_mismatch")?,
                unknown: count("unknown")?,
                unknown_attempts: count("unknown_attempts")?,
                retries: count("retries")?,
                exhausted: flag("exhausted")?,
                completed: flag("completed")?,
            })
        })
        .transpose()
    }

    /// At most one 1000-key S3 LIST page per invocation. Each page is committed
    /// atomically with its opaque cursor. A worker restart resumes at that cursor.
    /// None means another worker owns the lease or this cycle is finished.
    pub async fn scan_page(
        &self,
        store: &impl PrivateInventoryLister,
        scope: PrivateScope,
    ) -> Result<Option<InventoryProgress>, InventoryError> {
        let bucket = store.private_bucket();
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        sqlx::query(
            "INSERT INTO g09_private_inventory_scans(bucket,scope)
             VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(bucket)
        .bind(scope.kind())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        let claim: Option<(i64, Option<String>, i64)> = sqlx::query_as(
            "UPDATE g09_private_inventory_scans
                SET lease_epoch=lease_epoch+1,
                    leased_until=clock_timestamp()+interval '180 seconds'
              WHERE bucket=$1 AND scope=$2 AND NOT completed AND NOT exhausted
                AND (leased_until IS NULL OR leased_until<=clock_timestamp())
              RETURNING lease_epoch,continuation,cycle",
        )
        .bind(bucket)
        .bind(scope.kind())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        let Some((epoch, token, cycle)) = claim else {
            return Ok(None);
        };

        let fetched = store.list_private_page(scope, token.as_deref()).await;
        let page = match fetched {
            Ok(page) if valid_page(&page, token.as_deref(), scope) => page,
            _ => {
                self.note_unavailable(bucket, scope, epoch).await;
                return Err(InventoryError::Unavailable);
            }
        };

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        let owns: Option<i64> = sqlx::query_scalar(
            "SELECT lease_epoch FROM g09_private_inventory_scans
              WHERE bucket=$1 AND scope=$2 AND lease_epoch=$3
                AND leased_until>clock_timestamp() AND NOT completed
              FOR UPDATE",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(epoch)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        if owns.is_none() {
            tx.rollback()
                .await
                .map_err(|_| InventoryError::Unavailable)?;
            return Err(InventoryError::Busy);
        }

        let mut counts = [0_i64; 6];
        for object in &page.objects {
            let status = classify(&mut tx, bucket, scope, object).await?;
            let digest = Sha256::digest(object.key.as_bytes());
            let inserted = sqlx::query(
                "INSERT INTO g09_private_inventory_findings
                    (bucket,scope,cycle,key_sha256,status,size_bytes)
                 VALUES($1,$2,$3,$4,$5,$6)
                 ON CONFLICT DO NOTHING",
            )
            .bind(bucket)
            .bind(scope.kind())
            .bind(cycle)
            .bind(digest.as_slice())
            .bind(status)
            .bind(object.size_bytes.filter(|size| *size >= 0))
            .execute(&mut *tx)
            .await
            .map_err(|_| InventoryError::Unavailable)?
            .rows_affected();
            if inserted == 1 {
                counts[0] += 1;
                let index = match status {
                    "registered" => 1,
                    "recently_pending" => 2,
                    "unregistered_candidate" => 3,
                    "size_mismatch" => 4,
                    _ => 5,
                };
                counts[index] += 1;
            }
        }
        let completed = page.continuation.is_none();
        sqlx::query(
            "UPDATE g09_private_inventory_scans
                SET continuation=$4,scanned=scanned+$5,registered=registered+$6,
                    recently_pending=recently_pending+$7,
                    unregistered_candidates=unregistered_candidates+$8,
                    size_mismatch=size_mismatch+$9,unknown=unknown+$10,
                    completed=$11,leased_until=NULL,
                    consecutive_failures=0,checkpoint_at=clock_timestamp()
              WHERE bucket=$1 AND scope=$2 AND lease_epoch=$3",
        )
        .bind(bucket)
        .bind(scope.kind())
        .bind(epoch)
        .bind(page.continuation.as_deref())
        .bind(counts[0])
        .bind(counts[1])
        .bind(counts[2])
        .bind(counts[3])
        .bind(counts[4])
        .bind(counts[5])
        .bind(completed)
        .execute(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        tx.commit().await.map_err(|_| InventoryError::Unavailable)?;
        self.progress(bucket, scope).await
    }

    /// Begin another explicit read-only cycle, retaining previous findings.
    pub async fn begin_next_cycle(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<bool, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let changed = sqlx::query(
            "UPDATE g09_private_inventory_scans
                SET cycle=cycle+1,continuation=NULL,scanned=0,registered=0,
                    recently_pending=0,unregistered_candidates=0,
                    size_mismatch=0,unknown=0,checkpoint_at=NULL,
                    retries=0,unknown_attempts=0,consecutive_failures=0,
                    exhausted=FALSE,completed=FALSE
              WHERE bucket=$1 AND scope=$2 AND completed
                AND (leased_until IS NULL OR leased_until<=clock_timestamp())",
        )
        .bind(bucket)
        .bind(scope.kind())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        Ok(changed.rows_affected() == 1)
    }

    /// Any S3 LIST outage/invalid provider page counts as unknown, not an
    /// unregistered or missing file. The durable budget requires explicit
    /// operator recovery after eight consecutive failures.
    async fn note_unavailable(&self, bucket: &str, scope: PrivateScope, epoch: i64) {
        let _ = sqlx::query(
            "UPDATE g09_private_inventory_scans
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
        let updated = sqlx::query(
            "UPDATE g09_private_inventory_scans
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
}

fn valid_page(page: &PrivateInventoryPage, token: Option<&str>, scope: PrivateScope) -> bool {
    page.objects.len() <= 1000
        && page
            .continuation
            .as_deref()
            .is_none_or(|next| Some(next) != token)
        && page
            .objects
            .iter()
            .all(|object| object.key.starts_with(scope.prefix()))
}

async fn classify(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    bucket: &str,
    scope: PrivateScope,
    object: &crate::services::reconciliation::ListedPrivateObject,
) -> Result<&'static str, InventoryError> {
    if PrivateObjectKey::parse(&object.key).is_err()
        || object.size_bytes.is_none_or(|size| size < 0)
    {
        return Ok("unknown");
    }
    let row: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT state,size_bytes,object_kind FROM storage_objects
          WHERE bucket=$1 AND object_key=$2",
    )
    .bind(bucket)
    .bind(&object.key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| InventoryError::Unavailable)?;
    if let Some((state, size, kind)) = row {
        if kind != scope.kind() {
            return Ok("unknown");
        }
        if Some(size) != object.size_bytes {
            return Ok("size_mismatch");
        }
        return Ok(if state == "pending" {
            "recently_pending"
        } else {
            "registered"
        });
    }
    let Some(modified) = object.modified_at else {
        return Ok("unknown");
    };
    if SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
        < PUT_GRACE
    {
        return Ok("recently_pending");
    }
    let Some(id) = object
        .key
        .strip_prefix(scope.prefix())
        .and_then(|value| value.strip_suffix(".enc"))
    else {
        return Ok("unknown");
    };
    // A referenced private V1/free revision is never an unregistered orphan.
    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM chapter_versions
                       WHERE storage_object_id=$1::uuid)
             OR EXISTS(SELECT 1 FROM chapter_encrypted_release_events
                       WHERE storage_object_id=$1::uuid)",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| InventoryError::Unavailable)?;
    Ok(if referenced {
        "unknown"
    } else {
        "unregistered_candidate"
    })
}
