//! Live D-05 PACK browser acceptance: actual Rust router, PG18, MinIO Range,
//! Chromium and actor-scoped source fallback. No real cloud or persistent DB.
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::Command,
};

pub(super) async fn verify(
    app: &axum::Router,
    chapter: i64,
    version: i64,
    buyer_session: &str,
    outsider_session: &str,
    expected_text: &str,
) {
    let temp = std::env::var("RUNNER_TEMP").expect("disposable CI temp directory");
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    assert_eq!(
        std::env::var("TOR_TEST_S3_ENDPOINT").as_deref(),
        Ok(super::super::ENDPOINT)
    );
    let fixture_path = Path::new(&temp).join("d05-layer-pack-live-chromium.json");
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
    let mut file = options.open(&fixture_path).expect("fresh private fixture");
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("reserved D05 loopback HTTP port");
    let app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("live D05 PACK router");
    });
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/d05_layer_pack_live_chromium.cjs");
    assert!(script.is_file());
    let fixture_for_child = fixture_path.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "150s", "node"])
            .arg(script)
            .env("TOR_D05_PACK_CHROMIUM_FIXTURE", fixture_for_child)
            .output()
    })
    .await
    .expect("Chromium blocking runner");
    server.abort();
    let _ = server.await;
    fs::remove_file(&fixture_path).expect("remove private fixture");

    let output = output.expect("Node, timeout and Chromium installed in isolated CI");
    assert!(
        output.status.success(),
        "D05 PACK live Chromium failed (exit {:?}), secret diagnostics withheld",
        output.status.code()
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("D05 LIVE PACK Chromium PASS: desktop")
            && out.contains("D05 LIVE PACK Chromium PASS: mobile"),
        "both browser viewports must read one exact private PACK frame"
    );
}
