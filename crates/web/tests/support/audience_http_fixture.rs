//! Shared mechanics for the five legacy audience HTTP PostgreSQL test targets.
//! Database admission, SQL execution, lock targets and privacy assertions stay local.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request},
    response::Response,
    Router,
};
use std::process::Command;
use tower::ServiceExt;

pub(super) fn client(url: &str) -> Command {
    assert!(url.ends_with("/tor_ci_legacy_analytics") && url.contains("127.0.0.1:5432"));
    let mut command = Command::new("psql");
    command
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000 -c idle_in_transaction_session_timeout=10000",
        )
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url]);
    command
}

pub(super) async fn call(app: &Router, path: &str, cookie: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

pub(super) async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
