//! Private Prometheus endpoint and bounded process-local technical counters.
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::{sync::OnceLock, time::Duration};
use tor_observability::{
    domain::{RouteGroup, StatusGroup},
    services::HttpMetrics,
};

static METRICS: OnceLock<HttpMetrics> = OnceLock::new();

pub(crate) fn record_http(family: &str, status: u16, elapsed: Duration) {
    METRICS.get_or_init(HttpMetrics::default).observe(
        RouteGroup::from_ril_family(family),
        StatusGroup::from_http(status),
        elapsed,
    );
}

fn equal_secret(a: &[u8], b: &[u8]) -> bool {
    // Constant work over bounded, validated token sizes.
    if a.len() != b.len() || a.len() < 32 || a.len() > 256 {
        return false;
    }
    let mut difference: u8 = 0;
    for (&left, &right) in a.iter().zip(b) {
        difference |= left ^ right;
    }
    difference == 0
}

fn authorized(headers: &HeaderMap, token: Option<&str>) -> bool {
    let Some(token) = token.filter(|s| s.is_ascii()) else {
        return false;
    };
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    value
        .strip_prefix("Bearer ")
        .is_some_and(|received| equal_secret(received.as_bytes(), token.as_bytes()))
}

fn endpoint(headers: HeaderMap, token: Option<&str>, metrics: &HttpMetrics) -> Response {
    if !authorized(&headers, token) {
        // Identical response for absent config and wrong credential; no secrets echoed.
        return (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")], "").into_response();
    }
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (header::HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff")),
        ],
        metrics.prometheus(),
    )
        .into_response()
}

pub(crate) async fn metrics_endpoint(headers: HeaderMap) -> Response {
    let token = std::env::var("TOR_METRICS_TOKEN").ok();
    endpoint(headers, token.as_deref(), METRICS.get_or_init(HttpMetrics::default))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};

    fn headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn absent_or_wrong_credentials_fail_closed() {
        let metrics = HttpMetrics::default();
        let secret = "a".repeat(48);
        for (auth, expected) in [
            (HeaderMap::new(), Some(secret.as_str())),
            (headers(&"b".repeat(48)), Some(secret.as_str())),
            (headers(&secret), None),
            (headers("short"), Some("short")),
        ] {
            let response = endpoint(auth, expected, &metrics);
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }

    #[tokio::test]
    async fn secret_does_not_appear_in_metrics_or_response_headers() {
        let secret = "a".repeat(48);
        let metrics = HttpMetrics::default();
        metrics.observe(RouteGroup::Catalog, StatusGroup::Success, Duration::from_millis(40));
        let response = endpoint(headers(&secret), Some(&secret), &metrics);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let text = String::from_utf8(to_bytes(response.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
        assert!(text.contains("tor_http_request_duration_seconds_count"));
        assert!(!text.contains(&secret));
        assert!(!text.contains("authorization"));
    }
}
