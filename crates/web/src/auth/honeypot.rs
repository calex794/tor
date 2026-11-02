//! AB-03/04: fixed-cardinality, non-blocking decoys. No raw IP, email, path,
//! token or user-agent is retained. This is not an automatic-ban mechanism.
use axum::{
    http::{header, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::atomic::{AtomicU64, Ordering};

static LOGIN_HITS: AtomicU64 = AtomicU64::new(0);
static REGISTER_HITS: AtomicU64 = AtomicU64::new(0);
static ROUTE_HITS: AtomicU64 = AtomicU64::new(0);

pub(super) enum TrapKind {
    Login,
    Register,
    Route,
}

pub(super) fn filled(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.trim().is_empty())
}

pub(super) fn record(kind: TrapKind) {
    let counter = match kind {
        TrapKind::Login => &LOGIN_HITS,
        TrapKind::Register => &REGISTER_HITS,
        TrapKind::Route => &ROUTE_HITS,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn counts() -> (u64, u64, u64) {
    (
        LOGIN_HITS.load(Ordering::Relaxed),
        REGISTER_HITS.load(Ordering::Relaxed),
        ROUTE_HITS.load(Ordering::Relaxed),
    )
}

/// Deliberately never extracts the untrusted path or accesses the database.
/// The named wildcard matches NEwBOO's /__bp_honeypot/{trap_name:path}.
pub(super) async fn trap() -> Response {
    record(TrapKind::Route);
    (
        StatusCode::NOT_FOUND,
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                HeaderName::from_static("x-robots-tag"),
                HeaderValue::from_static("noindex, nofollow, noarchive"),
            ),
            (
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            ),
        ],
        "",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[test]
    fn blank_decoys_do_not_reject_real_forms() {
        assert!(!filled(None));
        assert!(!filled(Some("")));
        assert!(!filled(Some("  \t\n  ")));
        assert!(filled(Some("bot-filled")));
    }

    #[tokio::test]
    async fn trap_has_empty_private_response_without_path_or_identity() {
        let response = trap().await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()["x-robots-tag"],
            "noindex, nofollow, noarchive"
        );
        let body = to_bytes(response.into_body(), 64).await.unwrap();
        assert!(body.is_empty());
    }
}
