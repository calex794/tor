//! Recovery for one exact version after a lost S3 or PostgreSQL response.
use super::*;

#[derive(sqlx::FromRow)]
pub struct QuarantineRelease {
    pub bucket: String,
    pub object_key: String,
    pub object_version: String,
    pub etag: String,
    pub sha256: Vec<u8>,
    pub size_bytes: i64,
    pub backup_bucket: String,
    pub backup_key: String,
    pub backup_version: String,
    pub requested: bool,
    pub completed: bool,
}

impl QuarantineRelease {
    pub fn observation(&self) -> Result<QuarantineObservation<'_>, FreeQuarantineError> {
        let evidence = QuarantineObservation {
            bucket: &self.bucket,
            key: &self.object_key,
            version: &self.object_version,
            etag: &self.etag,
            sha256: self
                .sha256
                .as_slice()
                .try_into()
                .map_err(|_| FreeQuarantineError::Invalid)?,
            size: self
                .size_bytes
                .try_into()
                .map_err(|_| FreeQuarantineError::Invalid)?,
        };
        if !evidence.valid() {
            return Err(FreeQuarantineError::Invalid);
        }
        Ok(evidence)
    }
}

impl PgFreeKeyQuarantine {
    pub async fn find_release(
        &self,
        bucket: &str,
        key: &str,
        version: &str,
    ) -> Result<Option<QuarantineRelease>, FreeQuarantineError> {
        if !valid_bucket(bucket) || !free_key(key) || !version_id(version) {
            return Err(FreeQuarantineError::Invalid);
        }
        sqlx::query_as(
            "SELECT f.bucket,f.object_key,f.object_version,f.etag,f.sha256,f.size_bytes,
                    b.backup_bucket,b.backup_key,b.backup_version,
                    d.object_key IS NOT NULL AS requested,
                    r.object_key IS NOT NULL AS completed
             FROM free_storage_key_fences f
             JOIN free_storage_quarantine_rechecks b USING (bucket,object_key)
             LEFT JOIN free_storage_quarantine_delete_requests d USING (bucket,object_key)
             LEFT JOIN free_storage_quarantine_delete_receipts r USING (bucket,object_key)
             WHERE f.bucket=$1 AND f.object_key=$2 AND f.object_version=$3
               AND f.purpose='quarantine'",
        )
        .bind(bucket)
        .bind(key)
        .bind(version)
        .fetch_optional(&self.pool)
        .await
        .map_err(sql_error)
    }

    pub async fn request_retirement(
        &self,
        release: &QuarantineRelease,
    ) -> Result<QuarantineOutcome, FreeQuarantineError> {
        let evidence = release.observation()?;
        let mut tx = self.pool.begin().await.map_err(sql_error)?;
        Self::check_unreferenced(&mut tx, &evidence).await?;
        // Scope, primary version and approved backup converge on one
        // permanent key; all evidence is immutable at the database boundary.
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM free_storage_key_fences f
               JOIN free_storage_quarantine_rechecks b USING(bucket,object_key)
               WHERE f.bucket=$1 AND f.object_key=$2 AND f.purpose='quarantine'
                 AND f.object_version=$3 AND f.etag=$4 AND f.sha256=$5 AND f.size_bytes=$6
                 AND b.backup_bucket=$7 AND b.backup_key=$8 AND b.backup_version=$9)",
        )
        .bind(evidence.bucket)
        .bind(evidence.key)
        .bind(evidence.version)
        .bind(evidence.etag)
        .bind(evidence.sha256.as_slice())
        .bind(release.size_bytes)
        .bind(&release.backup_bucket)
        .bind(&release.backup_key)
        .bind(&release.backup_version)
        .fetch_one(&mut *tx)
        .await
        .map_err(sql_error)?;
        if !allowed {
            return Err(FreeQuarantineError::Protected);
        }
        let count = sqlx::query(
            "INSERT INTO free_storage_quarantine_delete_requests(bucket,object_key)
             VALUES($1,$2) ON CONFLICT(bucket,object_key) DO NOTHING",
        )
        .bind(evidence.bucket)
        .bind(evidence.key)
        .execute(&mut *tx)
        .await
        .map_err(sql_error)?
        .rows_affected();
        tx.commit().await.map_err(sql_error)?;
        Ok(if count == 1 {
            QuarantineOutcome::Recorded
        } else {
            QuarantineOutcome::AlreadyRecorded
        })
    }

    /// Called by G only after a version-specific lookup positively reports
    /// NoSuchVersion. The foreign key requires a pre-existing request.
    pub async fn record_retired(
        &self,
        observation: &QuarantineObservation<'_>,
    ) -> Result<(), FreeQuarantineError> {
        if !observation.valid() {
            return Err(FreeQuarantineError::Invalid);
        }
        let ready: bool = sqlx::query_scalar(
            "WITH matching_request AS (
                 SELECT f.bucket,f.object_key FROM free_storage_key_fences f
                 JOIN free_storage_quarantine_delete_requests d USING(bucket,object_key)
                 WHERE f.bucket=$1 AND f.object_key=$2 AND f.purpose='quarantine'
                   AND f.object_version=$3 AND f.etag=$4 AND f.sha256=$5 AND f.size_bytes=$6
             ), recorded AS (
                 INSERT INTO free_storage_quarantine_delete_receipts(bucket,object_key)
                 SELECT bucket,object_key FROM matching_request
                 ON CONFLICT(bucket,object_key) DO NOTHING
             )
             SELECT EXISTS(SELECT 1 FROM matching_request)",
        )
        .bind(observation.bucket)
        .bind(observation.key)
        .bind(observation.version)
        .bind(observation.etag)
        .bind(observation.sha256.as_slice())
        .bind(observation.size as i64)
        .fetch_one(&self.pool)
        .await
        .map_err(sql_error)?;
        if !ready {
            return Err(FreeQuarantineError::Protected);
        }
        Ok(())
    }
}
