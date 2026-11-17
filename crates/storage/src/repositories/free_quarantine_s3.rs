//! Object-store evidence for one exact version. No key-only DELETE fallback.
use aws_sdk_s3::{error::ProvideErrorMetadata, Client};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::free_expected_inventory_s3::valid_key;
use crate::domain::provider::{valid_private_bucket, StorageError};

#[derive(Clone, Eq, PartialEq)]
pub struct FreeVersionEvidence {
    pub version: String,
    pub etag: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

pub struct S3FreeQuarantineStore {
    client: Client,
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value != "null"
        && value.len() <= 1024
        && !value.chars().any(char::is_control)
}

fn drill_request(uri: &str, bucket: &str) -> bool {
    let Some((authority, path)) = uri
        .strip_prefix("http://")
        .or_else(|| uri.strip_prefix("https://"))
        .and_then(|rest| rest.split_once('/'))
    else {
        return false;
    };
    let loopback = authority == "127.0.0.1"
        || authority
            .strip_prefix("127.0.0.1:")
            .is_some_and(|port| port.parse::<u16>().is_ok_and(|number| number != 0));
    loopback
        && bucket.starts_with("tor-gc-drill-")
        && path.split_once('/').is_some_and(|(name, _)| name == bucket)
}

impl S3FreeQuarantineStore {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn read_exact_version(
        &self,
        bucket: &str,
        key: &str,
        version: &str,
    ) -> Result<Option<FreeVersionEvidence>, StorageError> {
        if !valid_private_bucket(bucket) || !valid_key(key) || !valid_version(version) {
            return Err(StorageError::Invalid);
        }
        let result = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .version_id(version)
            .send()
            .await;
        let object = match result {
            Ok(response) => response,
            Err(error)
                if error.as_service_error().and_then(|e| e.code()) == Some("NoSuchVersion")
                    && error
                        .raw_response()
                        .is_some_and(|r| r.status().as_u16() == 404) =>
            {
                return Ok(None);
            }
            Err(_) => return Err(StorageError::Unavailable),
        };
        if object.version_id() != Some(version) || object.content_encoding().is_some() {
            return Err(StorageError::Corrupt);
        }
        let size = object
            .content_length()
            .filter(|n| (1..=16_777_216).contains(n))
            .ok_or(StorageError::Corrupt)? as u64;
        let etag = object
            .e_tag()
            .filter(|tag| {
                (3..=256).contains(&tag.len())
                    && tag.starts_with('"')
                    && tag.ends_with('"')
                    && !tag.chars().any(char::is_control)
            })
            .ok_or(StorageError::Corrupt)?
            .to_owned();
        let mut body = object.body.into_async_read().take(size + 1);
        let mut count = 0_u64;
        let mut hash = Sha256::new();
        let mut buf = [0_u8; 8192];
        loop {
            let n = body
                .read(&mut buf)
                .await
                .map_err(|_| StorageError::Unavailable)?;
            if n == 0 {
                break;
            }
            count += n as u64;
            hash.update(&buf[..n]);
        }
        if count != size {
            return Err(StorageError::Corrupt);
        }
        Ok(Some(FreeVersionEvidence {
            version: version.to_owned(),
            etag,
            size,
            sha256: hash.finalize().into(),
        }))
    }

    /// The caller must have booked a PostgreSQL request and target an
    /// accepted provider. Always verify the exact requested version afterwards;
    /// a lost DELETE response can be retried against the same version.
    pub async fn remove_verified_version(
        &self,
        bucket: &str,
        key: &str,
        observed: &FreeVersionEvidence,
    ) -> Result<(), StorageError> {
        if !valid_private_bucket(bucket) || !valid_key(key) || !valid_version(&observed.version) {
            return Err(StorageError::Invalid);
        }
        let drill = bucket.to_owned();
        let _delete = self
            .client
            .delete_object()
            .bucket(bucket)
            .key(key)
            .version_id(&observed.version)
            .if_match(&observed.etag)
            .customize()
            .map_request(move |request| {
                if drill_request(request.uri(), &drill) {
                    Ok(request)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "DROP GC retirement outside isolated drill",
                    ))
                }
            })
            .send()
            .await;
        match self
            .read_exact_version(bucket, key, &observed.version)
            .await?
        {
            None => Ok(()),
            Some(_) => Err(StorageError::Unavailable),
        }
    }
}
