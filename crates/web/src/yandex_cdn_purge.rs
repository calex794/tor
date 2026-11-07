//! S025: Yandex Cloud CDN cache purge transport. Never log auth tokens,
//! full response bodies, or user-supplied URLs. Not wired to the live server.
use serde::Deserialize;
use std::time::Duration;
use tor_ingest::domain::cover::CoverError;

const CDN_HOST: &str = "https://cdn.api.cloud.yandex.net";
const OPERATION_HOST: &str = "https://operation.api.cloud.yandex.net";

#[derive(Debug, PartialEq, Eq)]
pub enum PurgeState {
    Running,
    Succeeded,
    Failed,
}

/// The authenticated management API is deliberately separate from the public
/// CDN hostname. Only the operator supplies the resource ID and bearer token.
pub struct YandexCdnPurge {
    client: reqwest::Client,
    resource_id: String,
    bearer: String,
    // The real binary always uses the fixed Yandex Cloud HTTPS management APIs.
    // Only unit tests may redirect HTTP traffic to a loopback test server.
    #[cfg(test)]
    test_cdn_endpoint: Option<String>,
    #[cfg(test)]
    test_operation_endpoint: Option<String>,
}

#[derive(Deserialize)]
struct CloudOperation {
    id: String,
    done: bool,
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    response: Option<serde_json::Value>,
}

fn valid_id(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Accept exactly one immutable S025 cover path. This prevents accidentally
/// purging all cached files (the Yandex API treats paths=[] as full purge).
fn validated_purge_path(path: &str) -> Result<(), CoverError> {
    if path.len() > 200
        || !path.starts_with("/public/covers/")
        || !path.ends_with(".webp*")
        || path.contains("//")
        || path.contains("..")
    {
        return Err(CoverError::Invalid);
    }
    let parts: Vec<_> = path
        .trim_start_matches("/public/covers/")
        .trim_end_matches(".webp*")
        .split('/')
        .collect();
    if parts.len() != 3
        || parts[0].parse::<u64>().ok().filter(|n| *n > 0).is_none()
        || parts[1].parse::<u64>().ok().filter(|n| *n > 0).is_none()
    {
        return Err(CoverError::Invalid);
    }
    let (variant, digest) = parts[2].split_once('-').ok_or(CoverError::Invalid)?;
    if !matches!(variant, "card" | "main")
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(CoverError::Invalid);
    }
    Ok(())
}

fn parse_operation(body: &str, expected_id: Option<&str>) -> Result<CloudOperation, CoverError> {
    let operation: CloudOperation =
        serde_json::from_str(body).map_err(|_| CoverError::Unavailable)?;
    if !valid_id(&operation.id, 128)
        || expected_id.is_some_and(|id| id != operation.id)
        || (operation.done && operation.error.is_none() == operation.response.is_none())
    {
        return Err(CoverError::Unavailable);
    }
    Ok(operation)
}

impl YandexCdnPurge {
    pub fn new(resource_id: String, bearer: String) -> Result<Self, CoverError> {
        if !valid_id(&resource_id, 50)
            || bearer.len() < 10
            || bearer.len() > 4096
            || bearer.bytes().any(|b| b.is_ascii_control() || b == b' ')
        {
            return Err(CoverError::Invalid);
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| CoverError::Unavailable)?;
        Ok(Self {
            client,
            resource_id,
            bearer,
            #[cfg(test)]
            test_cdn_endpoint: None,
            #[cfg(test)]
            test_operation_endpoint: None,
        })
    }

    fn cdn_endpoint(&self) -> &str {
        #[cfg(test)]
        if let Some(endpoint) = &self.test_cdn_endpoint {
            return endpoint;
        }
        CDN_HOST
    }

    fn operation_endpoint(&self) -> &str {
        #[cfg(test)]
        if let Some(endpoint) = &self.test_operation_endpoint {
            return endpoint;
        }
        OPERATION_HOST
    }

    /// Submit one *specific* file path. A missing response is an error, not
    /// evidence of success. The returned operation ID must be persisted in PG.
    pub async fn submit(&self, path: &str) -> Result<String, CoverError> {
        validated_purge_path(path)?;
        let url = format!("{}/cdn/v1/cache/{}:purge", self.cdn_endpoint(), self.resource_id);
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.bearer)
            .json(&serde_json::json!({ "paths": [path] }))
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        if !response.status().is_success() {
            return Err(CoverError::Unavailable);
        }
        let body = response.text().await.map_err(|_| CoverError::Unavailable)?;
        let operation = parse_operation(&body, None)?;
        if operation.error.is_some() {
            return Err(CoverError::Unavailable);
        }
        Ok(operation.id)
    }

    /// The operation is not complete merely because POST returned HTTP 200.
    /// Only done=true with a non-error response is considered success.
    pub async fn poll(&self, operation_id: &str) -> Result<PurgeState, CoverError> {
        if !valid_id(operation_id, 128) {
            return Err(CoverError::Invalid);
        }
        let url = format!("{}/operations/{operation_id}", self.operation_endpoint());
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        if !response.status().is_success() {
            return Err(CoverError::Unavailable);
        }
        let body = response.text().await.map_err(|_| CoverError::Unavailable)?;
        let operation = parse_operation(&body, Some(operation_id))?;
        Ok(if operation.error.is_some() {
            PurgeState::Failed
        } else if operation.done {
            PurgeState::Succeeded
        } else {
            PurgeState::Running
        })
    }
}

#[cfg(test)]
#[path = "yandex_cdn_purge/http_tests.rs"]
mod http_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_full_or_broad_cache_purge() {
        let digest = "ab".repeat(32);
        let good = format!("/public/covers/950/7/main-{digest}.webp*");
        assert!(validated_purge_path(&good).is_ok());
        for path in [
            "",
            "/",
            "/public/covers/*",
            "/public/covers/950/*",
            "/public/covers/950/7/card-xyz.webp*",
            "/public/covers/950/7/card-a.webp*",
            "/public/covers/950/7/../../paid.webp*",
            "/public/covers/950/7/main-.webp*",
            "/public/covers/950/7/main-1.webp",
        ] {
            assert!(validated_purge_path(path).is_err(), "{path}");
        }
    }

    #[test]
    fn operation_success_is_not_submission() {
        assert!(parse_operation(r#"{"id":"op-1","done":false}"#, None).is_ok());
        assert!(
            parse_operation(r#"{"id":"op-1","done":true,"response":{}}"#, Some("op-1")).is_ok()
        );
        assert!(parse_operation(
            r#"{"id":"op-1","done":true,"error":{"code":13}}"#,
            Some("op-1")
        )
        .is_ok());
        assert!(parse_operation(r#"{"id":"op-1","done":true}"#, None).is_err());
        assert!(parse_operation(
            r#"{"id":"op-1","done":true,"response":{},"error":{}}"#,
            None
        )
        .is_err());
        assert!(parse_operation(r#"{"id":"op-2","done":false}"#, Some("op-1")).is_err());
    }

    #[test]
    fn credentials_and_ids_are_strict() {
        assert!(YandexCdnPurge::new("res-1".into(), "safe-token-123".into()).is_ok());
        for id in ["", "../res", "res/other", "a?x=1", "a b"] {
            assert!(YandexCdnPurge::new(id.into(), "safe-token-123".into()).is_err());
        }
    }
}
