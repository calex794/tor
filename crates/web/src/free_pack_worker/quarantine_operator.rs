//! Internal one-object FREE quarantine workflow. Not routed by the site.
use tor_books::repositories::free_key_quarantine::{
    FreeQuarantineError, PgFreeKeyQuarantine, QuarantineBackup, QuarantineObservation,
    QuarantineOutcome,
};
use tor_storage::repositories::free_quarantine_s3::{FreeVersionEvidence, S3FreeQuarantineStore};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineOperatorError {
    Database(FreeQuarantineError),
    Unavailable,
    MissingVersion,
    EvidenceMismatch,
    ScopeDenied,
}

impl From<FreeQuarantineError> for QuarantineOperatorError {
    fn from(error: FreeQuarantineError) -> Self {
        Self::Database(error)
    }
}

pub struct VersionLocator<'a> {
    pub bucket: &'a str,
    pub key: &'a str,
    pub version: &'a str,
}

fn observation<'a>(
    locator: &'a VersionLocator<'a>,
    evidence: &'a FreeVersionEvidence,
) -> QuarantineObservation<'a> {
    QuarantineObservation {
        bucket: locator.bucket,
        key: locator.key,
        version: &evidence.version,
        etag: &evidence.etag,
        sha256: evidence.sha256,
        size: evidence.size,
    }
}

async fn required_read(
    store: &S3FreeQuarantineStore,
    locator: &VersionLocator<'_>,
) -> Result<FreeVersionEvidence, QuarantineOperatorError> {
    store
        .read_exact_version(locator.bucket, locator.key, locator.version)
        .await
        .map_err(|_| QuarantineOperatorError::Unavailable)?
        .ok_or(QuarantineOperatorError::MissingVersion)
}

pub async fn enter_quarantine(
    db: &PgFreeKeyQuarantine,
    primary: &S3FreeQuarantineStore,
    locator: &VersionLocator<'_>,
) -> Result<QuarantineOutcome, QuarantineOperatorError> {
    let exact = required_read(primary, locator).await?;
    Ok(db.enter(&observation(locator, &exact)).await?)
}

pub async fn recheck_quarantine(
    db: &PgFreeKeyQuarantine,
    primary: &S3FreeQuarantineStore,
    archive: &S3FreeQuarantineStore,
    locator: &VersionLocator<'_>,
    backup: &VersionLocator<'_>,
) -> Result<QuarantineOutcome, QuarantineOperatorError> {
    if locator.bucket == backup.bucket {
        return Err(QuarantineOperatorError::ScopeDenied);
    }
    let exact = required_read(primary, locator).await?;
    let copy = required_read(archive, backup).await?;
    if exact.sha256 != copy.sha256 || exact.size != copy.size {
        return Err(QuarantineOperatorError::EvidenceMismatch);
    }
    Ok(db
        .recheck(
            &observation(locator, &exact),
            &QuarantineBackup {
                bucket: backup.bucket,
                key: backup.key,
                version: backup.version,
            },
        )
        .await?)
}

fn drill_bucket(bucket: &str) -> bool {
    bucket.strip_prefix("tor-gc-drill-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

pub async fn retire_drill_object(
    db: &PgFreeKeyQuarantine,
    primary: &S3FreeQuarantineStore,
    archive: &S3FreeQuarantineStore,
    locator: &VersionLocator<'_>,
) -> Result<QuarantineOutcome, QuarantineOperatorError> {
    if !drill_bucket(locator.bucket) {
        return Err(QuarantineOperatorError::ScopeDenied);
    }
    let record = db
        .find_release(locator.bucket, locator.key, locator.version)
        .await?
        .ok_or(QuarantineOperatorError::MissingVersion)?;
    if !drill_bucket(&record.backup_bucket) {
        return Err(QuarantineOperatorError::ScopeDenied);
    }
    if record.completed {
        return Ok(QuarantineOutcome::AlreadyRecorded);
    }
    let frozen = record.observation()?;
    let backup = required_read(
        archive,
        &VersionLocator {
            bucket: &record.backup_bucket,
            key: &record.backup_key,
            version: &record.backup_version,
        },
    )
    .await?;
    if backup.sha256 != frozen.sha256 || backup.size != frozen.size {
        return Err(QuarantineOperatorError::EvidenceMismatch);
    }
    let current = primary
        .read_exact_version(locator.bucket, locator.key, locator.version)
        .await
        .map_err(|_| QuarantineOperatorError::Unavailable)?;
    match current {
        Some(exact)
            if (exact.sha256, exact.size, exact.etag.as_str())
                == (frozen.sha256, frozen.size, frozen.etag) =>
        {
            db.request_retirement(&record).await?;
            primary
                .remove_verified_version(locator.bucket, locator.key, &exact)
                .await
                .map_err(|_| QuarantineOperatorError::Unavailable)?;
        }
        None if record.requested => (),
        _ => return Err(QuarantineOperatorError::EvidenceMismatch),
    }
    db.record_retired(&frozen).await?;
    Ok(QuarantineOutcome::Recorded)
}
