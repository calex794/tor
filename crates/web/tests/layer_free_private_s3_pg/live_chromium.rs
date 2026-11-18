//! Live PG18 + MinIO + Rust HTTP + Chromium acceptance for archived FREE SINGLE.
//! Only synthetic runner credentials, never production or persistent storage.
use super::*;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub(super) async fn verify(
    app: &Router,
    chapter: i64,
    version: i64,
    buyer_session: &str,
    outsider_session: &str,
    expected_text: &str,
) {
    let temp = env::var("RUNNER_TEMP").expect("isolated CI temp directory");
    let fixture_path = Path::new(&temp).join("d05-layer-free-live-chromium.json");
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    assert!(
        env::var("TEST_DATABASE_URL")
            .expect("disposable PG")
            .ends_with("/tor_ci"),
        "never execute browser acceptance against persistent PG"
    );
    let payload = serde_json::json!({
        "chapter": chapter,
        "version": version,
        "buyer_session": buyer_session,
        "outsider_session": outsider_session,
        "expected_text": expected_text,
    });
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&fixture_path)
        .expect("fresh private browser fixture");
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);

    // The actual Rust Axum router is served over loopback, not an HTTP mock.
    // The browser then follows the authentic short-lived S3 signed URL.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("D05 loopback port reserved for this isolated CI test");
    let app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("live D05 HTTP router");
    });

    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/d05_layer_free_live_chromium.cjs");
    assert!(script.is_file(), "required real Chromium script");
    let fixture_for_child = fixture_path.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "150s", "node"])
            .arg(script)
            .env("TOR_D05_FREE_CHROMIUM_FIXTURE", fixture_for_child)
            .output()
    })
    .await
    .expect("Chromium runner blocking task");

    server.abort();
    let _ = server.await;
    fs::remove_file(&fixture_path).expect("delete session fixture");
    let output = output.expect("Linux timeout and Node/Chromium required");
    assert!(
        output.status.success(),
        "D05 live Chromium failed (code {:?}); secrets and S3 URLs withheld",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("D05 LIVE FREE Chromium PASS: desktop")
            && stdout.contains("D05 LIVE FREE Chromium PASS: mobile"),
        "both real Chromium viewport runs must complete against live D05 router"
    );
}
