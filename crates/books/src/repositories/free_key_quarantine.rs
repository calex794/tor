//! DROP-07h: durable, exact-key quarantine. No object-store mutation here.
use sqlx::PgPool;

mod retirement;
pub use retirement::QuarantineRelease;

use crate::services::authoring::AuthorError;

use super::{
    free_expected_inventory::{valid_bucket, valid_cursor},
    free_observed_reconcile::{FreeReferenceState, PgFreeObservedClassifier},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreeQuarantineError {
    Invalid,
    Unavailable,
    Protected,
}

impl FreeQuarantineError {
    pub fn into_author(self) -> AuthorError {
        match self {
            Self::Invalid => AuthorError::Invalid,
            Self::Unavailable => AuthorError::Unavailable,
            Self::Protected => AuthorError::Conflict,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineOutcome {
    Recorded,
    AlreadyRecorded,
}

pub struct QuarantineObservation<'a> {
    pub bucket: &'a str,
    pub key: &'a str,
    pub version: &'a str,
    pub etag: &'a str,
    pub sha256: [u8; 32],
    pub size: u64,
}

pub struct QuarantineBackup<'a> {
    pub bucket: &'a str,
    pub key: &'a str,
    pub version: &'a str,
}

pub struct PgFreeKeyQuarantine {
    pool: PgPool,
}

fn provider_field(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn version_id(value: &str) -> bool {
    provider_field(value, 1024) && value != "null"
}

fn free_key(value: &str) -> bool {
    fn positive(value: &str) -> bool {
        !value.starts_with('0')
            && value.bytes().all(|b| b.is_ascii_digit())
            && value.parse::<i64>().is_ok_and(|n| n > 0)
    }
    if !valid_cursor(value) {
        return false;
    }
    if let Some(single) = value.strip_prefix("private/free-chapters/") {
        return single
            .strip_suffix(".md")
            .and_then(|name| name.split_once('/'))
            .is_some_and(|(chapter, version)| positive(chapter) && positive(version));
    }
    value
        .strip_prefix("private/free-packs/")
        .and_then(|name| {
            name.strip_suffix(".zst")
                .or_else(|| name.strip_suffix(".manifest.json"))
        })
        .is_some_and(|stem| !stem.is_empty() && !stem.ends_with('/'))
}

impl QuarantineObservation<'_> {
    fn valid(&self) -> bool {
        valid_bucket(self.bucket)
            && free_key(self.key)
            && version_id(self.version)
            && provider_field(self.etag, 256)
            && self.etag.starts_with('"')
            && self.etag.ends_with('"')
            && self.etag.len() > 2
            && self.sha256 != [0; 32]
            && (1..=16_777_216).contains(&self.size)
    }
}

fn sql_error(error: sqlx::Error) -> FreeQuarantineError {
    if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23514") {
        FreeQuarantineError::Protected
    } else {
        FreeQuarantineError::Unavailable
    }
}

impl PgFreeKeyQuarantine {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Called before issuing any immutable PUT. The commit is durable before
    /// network I/O and stays protected across crashes and lost responses.
    pub async fn reserve_publication(
        &self,
        bucket: &str,
        keys: &[&str],
    ) -> Result<(), FreeQuarantineError> {
        if !valid_bucket(bucket)
            || keys.is_empty()
            || keys.len() > 500
            || keys.iter().any(|k| !free_key(k))
        {
            return Err(FreeQuarantineError::Invalid);
        }
        sqlx::query(
            r#"SELECT reserve_free_storage_publication($1,key)
             FROM (SELECT DISTINCT key COLLATE "C" AS key
                   FROM unnest($2::text[]) AS keys(key) ORDER BY 1) ordered"#,
        )
        .bind(bucket)
        .bind(keys)
        .execute(&self.pool)
        .await
        .map_err(sql_error)?;
        Ok(())
    }

    /// The caller must supply a full immutable S3 version observation. SQL
    /// claims the key first, then checks preexisting and in-flight references
    /// in the same transaction. Concurrent publishers use the same unique row.
    pub async fn enter(
        &self,
        observation: &QuarantineObservation<'_>,
    ) -> Result<QuarantineOutcome, FreeQuarantineError> {
        if !observation.valid() {
            return Err(FreeQuarantineError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(sql_error)?;
        let inserted = sqlx::query(
            "INSERT INTO free_storage_key_fences
               (bucket,object_key,purpose,object_version,etag,sha256,size_bytes)
             VALUES ($1,$2,'quarantine',$3,$4,$5,$6)
             ON CONFLICT (bucket,object_key) DO NOTHING",
        )
        .bind(observation.bucket)
        .bind(observation.key)
        .bind(observation.version)
        .bind(observation.etag)
        .bind(observation.sha256.as_slice())
        .bind(observation.size as i64)
        .execute(&mut *tx)
        .await
        .map_err(sql_error)?;
        if inserted.rows_affected() == 0 {
            let exact: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM free_storage_key_fences
                 WHERE bucket=$1 AND object_key=$2 AND purpose='quarantine'
                 AND object_version=$3 AND etag=$4 AND sha256=$5 AND size_bytes=$6)",
            )
            .bind(observation.bucket)
            .bind(observation.key)
            .bind(observation.version)
            .bind(observation.etag)
            .bind(observation.sha256.as_slice())
            .bind(observation.size as i64)
            .fetch_one(&mut *tx)
            .await
            .map_err(sql_error)?;
            return if exact {
                Ok(QuarantineOutcome::AlreadyRecorded)
            } else {
                Err(FreeQuarantineError::Protected)
            };
        }
        Self::check_unreferenced(&mut tx, observation).await?;
        tx.commit().await.map_err(sql_error)?;
        Ok(QuarantineOutcome::Recorded)
    }

    /// Recheck after a separately initiated read of both exact versions.
    /// Hold the permanent fence and recheck references before immutable audit.
    /// The result is evidence, not a physical-delete authorization.
    pub async fn recheck(
        &self,
        observation: &QuarantineObservation<'_>,
        backup: &QuarantineBackup<'_>,
    ) -> Result<QuarantineOutcome, FreeQuarantineError> {
        if !observation.valid()
            || !valid_bucket(backup.bucket)
            || backup.bucket == observation.bucket
            || !provider_field(backup.key, 1024)
            || !version_id(backup.version)
        {
            return Err(FreeQuarantineError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(sql_error)?;
        let ready: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM free_storage_key_fences
             WHERE bucket=$1 AND object_key=$2 AND purpose='quarantine'
             AND object_version=$3 AND etag=$4 AND sha256=$5 AND size_bytes=$6
             AND recorded_at <= clock_timestamp() - interval '24 hours')",
        )
        .bind(observation.bucket)
        .bind(observation.key)
        .bind(observation.version)
        .bind(observation.etag)
        .bind(observation.sha256.as_slice())
        .bind(observation.size as i64)
        .fetch_one(&mut *tx)
        .await
        .map_err(sql_error)?;
        if !ready {
            return Err(FreeQuarantineError::Protected);
        }
        Self::check_unreferenced(&mut tx, observation).await?;
        let inserted = sqlx::query(
            "INSERT INTO free_storage_quarantine_rechecks
               (bucket,object_key,backup_bucket,backup_key,backup_version)
             VALUES ($1,$2,$3,$4,$5) ON CONFLICT(bucket,object_key) DO NOTHING",
        )
        .bind(observation.bucket)
        .bind(observation.key)
        .bind(backup.bucket)
        .bind(backup.key)
        .bind(backup.version)
        .execute(&mut *tx)
        .await
        .map_err(sql_error)?;
        if inserted.rows_affected() == 0 {
            let exact: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM free_storage_quarantine_rechecks
                 WHERE bucket=$1 AND object_key=$2 AND backup_bucket=$3
                   AND backup_key=$4 AND backup_version=$5)",
            )
            .bind(observation.bucket)
            .bind(observation.key)
            .bind(backup.bucket)
            .bind(backup.key)
            .bind(backup.version)
            .fetch_one(&mut *tx)
            .await
            .map_err(sql_error)?;
            return if exact {
                Ok(QuarantineOutcome::AlreadyRecorded)
            } else {
                Err(FreeQuarantineError::Protected)
            };
        }
        tx.commit().await.map_err(sql_error)?;
        Ok(QuarantineOutcome::Recorded)
    }

    async fn check_unreferenced(
        conn: &mut sqlx::PgConnection,
        observation: &QuarantineObservation<'_>,
    ) -> Result<(), FreeQuarantineError> {
        let mut keys = vec![observation.key.to_owned()];
        if let Some(stem) = observation.key.strip_suffix(".zst") {
            keys.push(format!("{stem}.manifest.json"));
        } else if let Some(stem) = observation.key.strip_suffix(".manifest.json") {
            keys.push(format!("{stem}.zst"));
        }
        let references = PgFreeObservedClassifier::classify_in(conn, observation.bucket, &keys)
            .await
            .map_err(|_| FreeQuarantineError::Unavailable)?;
        if references
            .iter()
            .any(|r| r.state != FreeReferenceState::Unknown)
        {
            return Err(FreeQuarantineError::Protected);
        }
        Ok(())
    }
}

impl super::free_pack_attach::PgFreePackCatalog {
    pub async fn reserve_pack_destination(
        &self,
        target: &tor_contracts::drop_pack::PackPlacementTarget,
    ) -> Result<(), AuthorError> {
        PgFreeKeyQuarantine::new(self.pool.clone())
            .reserve_publication(&target.bucket, &[&target.pack_key, &target.manifest_key])
            .await
            .map_err(FreeQuarantineError::into_author)?;
        Ok(())
    }
}
