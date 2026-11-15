//! G-09 sanitized, bounded operator reporting. Never returns a raw S3 key.
use crate::{
    domain::provider::valid_private_bucket, repositories::reconciliation::InventoryError,
    services::reconciliation::PrivateScope,
};
use sqlx::{PgPool, Row};

#[derive(Debug)]
pub struct Finding {
    pub fingerprint: String,
    pub status: String,
    pub has_ref: Option<bool>,
    pub size_bytes: Option<i64>,
    pub observed: String,
}

#[derive(Debug)]
pub struct Checkpoint {
    pub cycle: i64,
    pub checkpoint: Option<String>,
    pub leased: bool,
}

pub struct PgG09Operator {
    pool: PgPool,
}

impl PgG09Operator {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn checkpoint(
        &self,
        bucket: &str,
        scope: PrivateScope,
        reverse: bool,
    ) -> Result<Option<Checkpoint>, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let sql = if reverse {
            "SELECT cycle, to_char(checkpoint_at AT TIME ZONE 'UTC',
                'YYYY-MM-DD HH24:MI:SS') AS checkpoint,
                coalesce(leased_until>clock_timestamp(),FALSE) AS leased
               FROM g09_registered_object_scans WHERE bucket=$1 AND scope=$2"
        } else {
            "SELECT cycle, to_char(checkpoint_at AT TIME ZONE 'UTC',
                'YYYY-MM-DD HH24:MI:SS') AS checkpoint,
                coalesce(leased_until>clock_timestamp(),FALSE) AS leased
               FROM g09_private_inventory_scans WHERE bucket=$1 AND scope=$2"
        };
        let row = sqlx::query(sql)
            .bind(bucket)
            .bind(scope.kind())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        row.map(|v| {
            Ok(Checkpoint {
                cycle: v
                    .try_get("cycle")
                    .map_err(|_| InventoryError::Unavailable)?,
                checkpoint: v
                    .try_get("checkpoint")
                    .map_err(|_| InventoryError::Unavailable)?,
                leased: v
                    .try_get("leased")
                    .map_err(|_| InventoryError::Unavailable)?,
            })
        })
        .transpose()
    }

    pub async fn findings(
        &self,
        bucket: &str,
        scope: PrivateScope,
        reverse: bool,
    ) -> Result<Vec<Finding>, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        // The fingerprint is a truncated one-way hash. No object ID, path, or token.
        let sql = if reverse {
            "SELECT substring(encode(key_sha256,'hex') from 1 for 24) AS fingerprint,
                    status,size_bytes,has_ref,
                    to_char(observed_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') AS observed
               FROM g09_registered_object_findings
              WHERE bucket=$1 AND scope=$2
                AND cycle=(SELECT cycle FROM g09_registered_object_scans
                            WHERE bucket=$1 AND scope=$2)
                AND status <> 'healthy'
              ORDER BY observed_at DESC,key_sha256 LIMIT 40"
        } else {
            "SELECT substring(encode(key_sha256,'hex') from 1 for 24) AS fingerprint,
                    status,size_bytes,NULL::boolean AS has_ref,
                    to_char(observed_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') AS observed
               FROM g09_private_inventory_findings
              WHERE bucket=$1 AND scope=$2
                AND cycle=(SELECT cycle FROM g09_private_inventory_scans
                            WHERE bucket=$1 AND scope=$2)
                AND status <> 'registered'
              ORDER BY observed_at DESC,key_sha256 LIMIT 40"
        };
        let rows = sqlx::query(sql)
            .bind(bucket)
            .bind(scope.kind())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        rows.into_iter()
            .map(|v| {
                Ok(Finding {
                    fingerprint: v
                        .try_get("fingerprint")
                        .map_err(|_| InventoryError::Unavailable)?,
                    status: v
                        .try_get("status")
                        .map_err(|_| InventoryError::Unavailable)?,
                    size_bytes: v
                        .try_get("size_bytes")
                        .map_err(|_| InventoryError::Unavailable)?,
                    has_ref: v
                        .try_get("has_ref")
                        .map_err(|_| InventoryError::Unavailable)?,
                    observed: v
                        .try_get("observed")
                        .map_err(|_| InventoryError::Unavailable)?,
                })
            })
            .collect()
    }

    /// Read-only GET/SHA-256 checkpoint for an explicitly started sample.
    pub async fn body_checkpoint(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<Option<Checkpoint>, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let row = sqlx::query(
            "SELECT cycle,to_char(checkpoint_at AT TIME ZONE 'UTC',
                      'YYYY-MM-DD HH24:MI:SS') AS checkpoint,
                    coalesce(leased_until>clock_timestamp(),FALSE) AS leased
               FROM g09_body_sha_scans WHERE bucket=$1 AND scope=$2",
        )
        .bind(bucket)
        .bind(scope.kind())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        row.map(|v| {
            Ok(Checkpoint {
                cycle: v
                    .try_get("cycle")
                    .map_err(|_| InventoryError::Unavailable)?,
                checkpoint: v
                    .try_get("checkpoint")
                    .map_err(|_| InventoryError::Unavailable)?,
                leased: v
                    .try_get("leased")
                    .map_err(|_| InventoryError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// Only pseudonymous anomaly hashes. A healthy sample is only counted.
    pub async fn body_findings(
        &self,
        bucket: &str,
        scope: PrivateScope,
    ) -> Result<Vec<Finding>, InventoryError> {
        if !valid_private_bucket(bucket) {
            return Err(InventoryError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT substring(encode(key_sha256,'hex') from 1 for 24) AS fingerprint,
                    status,size_bytes,NULL::boolean AS has_ref,
                    to_char(observed_at AT TIME ZONE 'UTC',
                      'YYYY-MM-DD HH24:MI:SS') AS observed
               FROM g09_body_sha_findings
              WHERE bucket=$1 AND scope=$2
                AND cycle=(SELECT cycle FROM g09_body_sha_scans
                            WHERE bucket=$1 AND scope=$2)
              ORDER BY observed_at DESC,key_sha256 LIMIT 40",
        )
        .bind(bucket)
        .bind(scope.kind())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        rows.into_iter()
            .map(|v| {
                Ok(Finding {
                    fingerprint: v
                        .try_get("fingerprint")
                        .map_err(|_| InventoryError::Unavailable)?,
                    status: v
                        .try_get("status")
                        .map_err(|_| InventoryError::Unavailable)?,
                    size_bytes: v
                        .try_get("size_bytes")
                        .map_err(|_| InventoryError::Unavailable)?,
                    has_ref: v
                        .try_get("has_ref")
                        .map_err(|_| InventoryError::Unavailable)?,
                    observed: v
                        .try_get("observed")
                        .map_err(|_| InventoryError::Unavailable)?,
                })
            })
            .collect()
    }
}
