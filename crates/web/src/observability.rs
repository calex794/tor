//! Private Prometheus endpoint and bounded process-local technical counters.
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tor_observability::{
    domain::{RouteGroup, StatusGroup},
    services::{
        auth::{LoginMetrics, LoginOutcome, LoginSnapshot},
        trace::{RequestTrace, SqlEvidence, TraceBuffer},
        HttpMetrics,
    },
};

static METRICS: OnceLock<HttpMetrics> = OnceLock::new();
static LOGIN_METRICS: OnceLock<LoginMetrics> = OnceLock::new();
static TRACES: OnceLock<TraceBuffer> = OnceLock::new();
static TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
#[cfg(unix)]
static PUBLISHER: OnceLock<tor_observability::services::publisher::TracePublisher> =
    OnceLock::new();

#[cfg(unix)]
fn publisher() -> &'static tor_observability::services::publisher::TracePublisher {
    PUBLISHER.get_or_init(|| {
        let configured = std::env::var_os("TOR_TRACE_STORE_SOCKET").map(std::path::PathBuf::from);
        tor_observability::services::publisher::TracePublisher::new(configured)
    })
}

/// Server-generated technical correlation only; never derived from client input.
#[derive(Clone, Copy)]
pub(crate) struct TraceId(pub(crate) u128);

pub(crate) fn new_trace_id() -> TraceId {
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let sequence = TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    TraceId(((tick as u128) << 64) | u128::from(sequence))
}

pub(crate) fn finish_trace(
    id: TraceId,
    family: &str,
    status: u16,
    elapsed: Duration,
    sqlx: Option<SqlEvidence>,
) -> RequestTrace {
    let trace = RequestTrace {
        trace_id: id.0,
        route: RouteGroup::from_ril_family(family),
        status: StatusGroup::from_http(status),
        duration_us: elapsed.as_micros().min(u64::MAX as u128) as u64,
        sqlx,
    };
    let retained = TRACES
        .get_or_init(TraceBuffer::default)
        .record(trace.clone());
    #[cfg(unix)]
    if retained {
        let _ = publisher().try_publish(&trace);
    }
    #[cfg(not(unix))]
    let _ = retained;
    trace
}

/// The operator can inspect these without an HTTP route or user identity.
pub(crate) fn snapshot_traces() -> Vec<RequestTrace> {
    TRACES.get_or_init(TraceBuffer::default).snapshot()
}

/// Operators only: process-local anonymous counters, never request paths.
pub(crate) fn http_summary() -> tor_observability::services::HttpSummary {
    METRICS.get_or_init(HttpMetrics::default).summary()
}

#[cfg(unix)]
pub(crate) fn trace_delivery_counts(
) -> Option<tor_observability::services::publisher::PublisherCounts> {
    PUBLISHER.get().map(|publisher| publisher.counts())
}

pub(crate) fn record_http(family: &str, status: u16, elapsed: Duration) {
    METRICS.get_or_init(HttpMetrics::default).observe(
        RouteGroup::from_ril_family(family),
        StatusGroup::from_http(status),
        elapsed,
    );
}

/// One low-cost counter per completed login attempt, never a user dimension.
pub(crate) fn record_login_outcome(outcome: LoginOutcome) {
    LOGIN_METRICS
        .get_or_init(LoginMetrics::default)
        .record(outcome);
}

pub(crate) fn login_summary() -> LoginSnapshot {
    LOGIN_METRICS.get_or_init(LoginMetrics::default).snapshot()
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

fn collected_metrics(metrics: &HttpMetrics) -> String {
    let mut text = metrics.prometheus();
    text.push_str(
        &LOGIN_METRICS
            .get_or_init(LoginMetrics::default)
            .prometheus(),
    );
    text.push_str("# HELP tor_auth_observation_dropped_total Private auth diagnostic events discarded (no identity labels).\n");
    text.push_str("# TYPE tor_auth_observation_dropped_total counter\n");
    text.push_str(&format!(
        "tor_auth_observation_dropped_total {}\n",
        crate::auth::auth_observation_dropped()
    ));
    #[cfg(unix)]
    {
        let counts = PUBLISHER.get().map(|p| p.counts()).unwrap_or_default();
        text.push_str(
            "# HELP tor_trace_export_sent_total Successfully acknowledged trace frames.\n\
             # TYPE tor_trace_export_sent_total counter\n",
        );
        text.push_str(&format!("tor_trace_export_sent_total {}\n", counts.sent));
        text.push_str(
            "# HELP tor_trace_export_failed_total Trace-store socket or database failures.\n\
             # TYPE tor_trace_export_failed_total counter\n",
        );
        text.push_str(&format!(
            "tor_trace_export_failed_total {}\n",
            counts.failed
        ));
        text.push_str(
            "# HELP tor_trace_export_dropped_total Queue saturation and invalid frames.\n\
             # TYPE tor_trace_export_dropped_total counter\n",
        );
        text.push_str(&format!(
            "tor_trace_export_dropped_total {}\n",
            counts.dropped
        ));
    }
    text
}

fn endpoint(headers: HeaderMap, token: Option<&str>, metrics: &HttpMetrics) -> Response {
    if !authorized(&headers, token) {
        // Identical response for absent config and wrong credential; no secrets echoed.
        return (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "no-store")],
            "",
        )
            .into_response();
    }
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            ),
        ],
        collected_metrics(metrics),
    )
        .into_response()
}

pub(crate) async fn metrics_endpoint(headers: HeaderMap) -> Response {
    let token = std::env::var("TOR_METRICS_TOKEN").ok();
    endpoint(
        headers,
        token.as_deref(),
        METRICS.get_or_init(HttpMetrics::default),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

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
    async fn login_outcome_series_are_also_protected_and_never_contain_identity() {
        let secret = "o".repeat(48);
        let metrics = HttpMetrics::default();
        let denied = endpoint(HeaderMap::new(), Some(&secret), &metrics);
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
        let response = endpoint(headers(&secret), Some(&secret), &metrics);
        let body = to_bytes(response.into_body(), 100_000).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("tor_auth_login_outcomes_total{outcome=\"lookup_unavailable\"}"));
        assert!(text.contains("tor_auth_login_outcomes_total{outcome=\"rate_limited\"}"));
        for forbidden in ["email=", "user_id", "cookie=", "request_id", "sql_text"] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
    }

    #[tokio::test]
    async fn secret_does_not_appear_in_metrics_or_response_headers() {
        let secret = "a".repeat(48);
        let metrics = HttpMetrics::default();
        metrics.observe(
            RouteGroup::Catalog,
            StatusGroup::Success,
            Duration::from_millis(40),
        );
        let response = endpoint(headers(&secret), Some(&secret), &metrics);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let text = String::from_utf8(
            to_bytes(response.into_body(), 100_000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(text.contains("tor_http_request_duration_seconds_count"));
        assert!(!text.contains(&secret));
        assert!(!text.contains("authorization"));
    }
}
