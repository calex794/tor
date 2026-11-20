//! UI-10B: live S034 PostgreSQL + MinIO mixed Book before deletion.
use super::{html_body, private_get, MARKDOWN};
use axum::{http::StatusCode, Router};
use std::{env, path::Path, process::Command};

pub(super) async fn verify(
    app: &Router,
    url: &str,
    book: i64,
    paid: [i64; 2],
    free: [i64; 2],
    buyer_cookie: &str,
) {
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    assert!(book > 0 && paid.iter().chain(free.iter()).all(|id| *id > 0));
    let uri = format!("/books/{book}");
    let response = private_get(app, &uri, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let csp = response.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("script-src 'none'") && !csp.contains("unsafe-inline"));
    let html = html_body(response).await;
    assert!(html.contains("data-tor-style=\"showcase\""));
    assert_eq!(html.matches("class=\"book-hero-card__title\"").count(), 1);
    assert!(html.matches("chapter-toc__item--paid").count() >= 2);
    assert!(html.matches("chapter-toc__badge--free").count() >= 2);
    for id in paid.into_iter().chain(free) {
        assert!(html.contains(&format!("href=\"/reader/{id}\"")), "missing chapter link");
    }
    for secret in [
        MARKDOWN, "Second paid edition content", "New V2, not purchased",
        "dek_b64u", "X-Amz-Signature", "data-paid-chapter-reader", "<img src=x onerror=",
    ] {
        assert!(!html.contains(secret), "public Book leaked paid content");
    }
    let buyer = private_get(app, &uri, Some(buyer_cookie)).await;
    assert_eq!(buyer.status(), StatusCode::OK);
    let buyer_html = html_body(buyer).await;
    for secret in [MARKDOWN, "dek_b64u", "X-Amz-Signature"] {
        assert!(!buyer_html.contains(secret), "buyer public metadata leaked private data");
    }
    for id in paid.into_iter().chain(free) {
        assert!(buyer_html.contains(&format!("href=\"/reader/{id}\"")));
    }
    assert!(Path::new(&env::var("RUNNER_TEMP").expect("disposable CI only")).is_dir());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3119").await.unwrap();
    let router = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("live mixed Book Axum router");
    });
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/ui10b_mixed_public_book_chromium.cjs");
    assert!(script.is_file());
    // Public numeric IDs only; NEVER pass buyer sessions, signed S3 URLs or KEKs.
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "110s", "node"])
            .arg(script)
            .env("TOR_UI10B_BOOK_ID", book.to_string())
            .env("TOR_UI10B_PAID_IDS", format!("{},{}", paid[0], paid[1]))
            .env("TOR_UI10B_FREE_IDS", format!("{},{}", free[0], free[1]))
            .output()
    })
    .await
    .unwrap()
    .expect("Node/timeout needed for Chromium");
    server.abort();
    let _ = server.await;
    assert!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("UI-10B MIXED PUBLIC PASS"),
        "UI-10B mixed public page acceptance failed (private diagnostics withheld)"
    );
}
