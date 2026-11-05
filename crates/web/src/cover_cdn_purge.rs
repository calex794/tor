//! S025/G: selective Yandex Cloud CDN invalidation for withdrawn public covers.
//! An origin S3 DELETE and an accepted CDN purge request are NOT a completed purge.
//! The worker is deliberately not started by the application.
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::{json, Value};
use tor_books::repositories::cover_publications::PgPublicCoverPublications;

const PURGE_ENDPOINT: &str = "https://cdn.api.cloud.yandex.net/cdn/v1/cache";
const OPERATION_ENDPOINT: &str = "https://operation.api.cloud.yandex.net/operations";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PurgeStatus {
    Pending,
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CdnPurgeError {
    Invalid,
    Unavailable,
    LostLease,
}

#[async_trait]
pub trait CoverCdnApi: Send + Sync {
    async fn submit(&self, exact_path: &str) -> Result<String, CdnPurgeError>;
    async fn status(&self, operation_id: &str) -> Result<PurgeStatus, CdnPurgeError>;
}

/// Fixed official endpoints: a production resource/token cannot redirect a
/// privileged purge request to an arbitrary destination.
pub struct YandexCoverCdnApi {
    client: reqwest::Client,
    resource_id: String,
    iam_token: String,
}

fn safe_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn safe_cover_path(path: &str) -> bool {
    path.starts_with("/public/covers/")
        && path.ends_with(".webp*")
        && path.matches('*').count() == 1
        && !path.contains("..")
        && path.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'*')
        })
}

fn operation_status(value: &Value, expected_id: &str) -> Result<PurgeStatus, CdnPurgeError> {
    if value.get("id").and_then(Value::as_str) != Some(expected_id) {
        return Err(CdnPurgeError::Unavailable);
    }
    let done = value
        .get("done")
        .and_then(Value::as_bool)
        .ok_or(CdnPurgeError::Unavailable)?;
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Ok(PurgeStatus::Failed);
    }
    if !done {
        return Ok(PurgeStatus::Pending);
    }
    if value.get("response").is_some_and(|response| !response.is_null()) {
        Ok(PurgeStatus::Success)
    } else {
        // Never mark a withdrawn public URL as purged on a malformed response.
        Err(CdnPurgeError::Unavailable)
    }
}

impl YandexCoverCdnApi {
    pub fn new(resource_id: String, iam_token: String) -> Result<Self, CdnPurgeError> {
        if !safe_identifier(&resource_id, 50)
            || iam_token.is_empty()
            || iam_token.len() > 8192
            || iam_token.chars().any(char::is_whitespace)
        {
            return Err(CdnPurgeError::Invalid);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| CdnPurgeError::Unavailable)?;
        Ok(Self {
            client,
            resource_id,
            iam_token,
        })
    }

    async fn decode(response: reqwest::Response) -> Result<Value, CdnPurgeError> {
        response
            .error_for_status()
            .map_err(|_| CdnPurgeError::Unavailable)?
            .json::<Value>()
            .await
            .map_err(|_| CdnPurgeError::Unavailable)
    }
}

#[async_trait]
impl CoverCdnApi for YandexCoverCdnApi {
    async fn submit(&self, exact_path: &str) -> Result<String, CdnPurgeError> {
        if !safe_cover_path(exact_path) {
            return Err(CdnPurgeError::Invalid);
        }
        // Never send an empty paths array: Yandex interprets it as FULL purge.
        let response = self
            .client
            .post(format!("{PURGE_ENDPOINT}/{}:purge", self.resource_id))
            .bearer_auth(&self.iam_token)
            .json(&json!({"paths": [exact_path]}))
            .send()
            .await
            .map_err(|_| CdnPurgeError::Unavailable)?;
        let value = Self::decode(response).await?;
        let operation_id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or(CdnPurgeError::Unavailable)?;
        if !safe_identifier(operation_id, 128)
            || value.get("error").is_some_and(|error| !error.is_null())
        {
            return Err(CdnPurgeError::Unavailable);
        }
        Ok(operation_id.to_owned())
    }

    async fn status(&self, operation_id: &str) -> Result<PurgeStatus, CdnPurgeError> {
        if !safe_identifier(operation_id, 128) {
            return Err(CdnPurgeError::Invalid);
        }
        let response = self
            .client
            .get(format!("{OPERATION_ENDPOINT}/{operation_id}"))
            .bearer_auth(&self.iam_token)
            .send()
            .await
            .map_err(|_| CdnPurgeError::Unavailable)?;
        operation_status(&Self::decode(response).await?, operation_id)
    }
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct CdnPurgeBatch {
    pub claimed: usize,
    pub submitted: usize,
    pub completed: usize,
    pub deferred: usize,
    pub retry_scheduled: usize,
}

pub struct CoverCdnPurgeWorker {
    ledger: PgPublicCoverPublications,
    api: Arc<dyn CoverCdnApi>,
}

impl CoverCdnPurgeWorker {
    pub fn new(ledger: PgPublicCoverPublications, api: Arc<dyn CoverCdnApi>) -> Self {
        Self { ledger, api }
    }

    /// A bounded, explicitly invoked stage. Retrying an ambiguous submit is
    /// safe: purging the same immutable path twice never broadens its scope.
    pub async fn submit_batch(&self) -> Result<CdnPurgeBatch, CdnPurgeError> {
        let claims = self
            .ledger
            .claim_purge_submissions(3)
            .await
            .map_err(|_| CdnPurgeError::Unavailable)?;
        let mut result = CdnPurgeBatch {
            claimed: claims.len(),
            ..CdnPurgeBatch::default()
        };
        for item in claims {
            let attempt = tokio::time::timeout(
                Duration::from_secs(12),
                self.api.submit(&item.purge_path()),
            )
            .await;
            match attempt {
                Ok(Ok(operation_id)) => {
                    if !self
                        .ledger
                        .purge_submitted_fenced(item.id, item.lease_epoch, &operation_id)
                        .await
                        .map_err(|_| CdnPurgeError::Unavailable)?
                    {
                        return Err(CdnPurgeError::LostLease);
                    }
                    result.submitted += 1;
                }
                _ => {
                    if !self
                        .ledger
                        .retry_purge_submission(item.id, item.lease_epoch)
                        .await
                        .map_err(|_| CdnPurgeError::Unavailable)?
                    {
                        return Err(CdnPurgeError::LostLease);
                    }
                    result.retry_scheduled += 1;
                }
            }
        }
        Ok(result)
    }

    /// Only a confirmed successful Operation.Get can mark an object purged.
    /// Pending/unknown results keep the operation ID for later polling.
    pub async fn poll_batch(&self) -> Result<CdnPurgeBatch, CdnPurgeError> {
        let claims = self
            .ledger
            .claim_purge_polls(3)
            .await
            .map_err(|_| CdnPurgeError::Unavailable)?;
        let mut result = CdnPurgeBatch {
            claimed: claims.len(),
            ..CdnPurgeBatch::default()
        };
        for (item, operation_id) in claims {
            let response = tokio::time::timeout(
                Duration::from_secs(12),
                self.api.status(&operation_id),
            )
            .await;
            let changed = match response {
                Ok(Ok(PurgeStatus::Success)) => {
                    let updated = self
                        .ledger
                        .purge_completed_fenced(item.id, item.lease_epoch, &operation_id)
                        .await;
                    result.completed += 1;
                    updated
                }
                Ok(Ok(PurgeStatus::Failed)) => {
                    let updated = self
                        .ledger
                        .purge_failed_fenced(item.id, item.lease_epoch, &operation_id)
                        .await;
                    result.retry_scheduled += 1;
                    updated
                }
                _ => {
                    let updated = self
                        .ledger
                        .defer_purge_poll(item.id, item.lease_epoch, &operation_id)
                        .await;
                    result.deferred += 1;
                    updated
                }
            }
            .map_err(|_| CdnPurgeError::Unavailable)?;
            if !changed {
                return Err(CdnPurgeError::LostLease);
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{operation_status, safe_cover_path, safe_identifier, CdnPurgeError, PurgeStatus};
    use serde_json::json;

    #[test]
    fn only_exact_immutable_cover_paths_not_full_cache() {
        assert!(safe_cover_path(
            "/public/covers/950/7/card-abcdef1234567890.webp*"
        ));
        for path in [
            "",
            "/",
            "/*",
            "/public/covers/*",
            "/public/covers/1/2/card.webp",
            "/public/covers/1/../card.webp*",
            "/public/covers/1/2/a.webp**",
            "https://evil.example/public/covers/1/2/a.webp*",
        ] {
            assert!(!safe_cover_path(path), "{path}");
        }
        assert!(safe_identifier("fcmq0j5033e516c56ctq", 128));
        assert!(!safe_identifier("id/../other", 128));
    }

    #[test]
    fn yandex_operation_must_finish_successfully_and_match_id() {
        assert_eq!(
            operation_status(&json!({"id":"op","done":false}), "op"),
            Ok(PurgeStatus::Pending)
        );
        assert_eq!(
            operation_status(&json!({"id":"op","done":true,"response":{}}), "op"),
            Ok(PurgeStatus::Success)
        );
        assert_eq!(
            operation_status(
                &json!({"id":"op","done":true,"error":{"code":13,"message":"failed"}}),
                "op"
            ),
            Ok(PurgeStatus::Failed)
        );
        assert_eq!(
            operation_status(&json!({"id":"other","done":true,"response":{}}), "op"),
            Err(CdnPurgeError::Unavailable)
        );
        assert_eq!(
            operation_status(&json!({"id":"op","done":true}), "op"),
            Err(CdnPurgeError::Unavailable)
        );
    }
}
