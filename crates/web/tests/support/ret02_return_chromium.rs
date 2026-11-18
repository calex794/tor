//! Browser E2E for RET-02, run by the existing C-15 disposable PG fixture.
//! Isolates the loopback server; no production URL, secrets, or external traffic.
use axum::Router;
use std::process::Command;

pub async fn verify(
    app: &Router,
    origin: &str,
    reader_session: &str,
    observer_session: &str,
    csrf: &str,
    chapter_id: i64,
    version_id: i64,
) {
    assert_eq!(origin, "http://127.0.0.1:3098");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("loopback Chromium reader port");
    let browser_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            browser_app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let (origin, reader_session, observer_session, csrf) = (
        origin.to_owned(),
        reader_session.to_owned(),
        observer_session.to_owned(),
        csrf.to_owned(),
    );
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["--signal=TERM", "--kill-after=5s", "120s", "node"])
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tools/ret02_return_chromium.cjs"
            ))
            .env("RET02_ORIGIN", origin)
            .env("RET02_SESSION_A", reader_session)
            .env("RET02_SESSION_B", observer_session)
            .env("RET02_CSRF", csrf)
            .env("RET02_CHAPTER", chapter_id.to_string())
            .env("RET02_VERSION", version_id.to_string())
            .output()
            .expect("Node/Playwright/Chromium from PG CI")
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
    assert!(
        output.status.success(),
        "RET-02 browser failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("RET-02 Chromium PASS"),
        "missing witnessed desktop/mobile RET-02 browser assertion"
    );
}
