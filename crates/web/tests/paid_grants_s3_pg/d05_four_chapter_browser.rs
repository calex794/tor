//! D-05/#1022: one real PG18 + MinIO + Rust HTTP + Chromium journey
//! reading two S3 FREE and two encrypted PAID chapters of the SAME deleted Book.
use super::d05_four_chapter_free::{FREE_ALPHA, FREE_BETA};
use super::{ENDPOINT, ORIGIN};
use axum::Router;
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::Command,
};

pub(super) async fn verify(
    app: &Router,
    old_session: &str,
    new_session: &str,
    free: [i64; 2],
    paid: [i64; 2],
) {
    let temp = env::var("RUNNER_TEMP").expect("disposable CI temp directory");
    let url = env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    assert_eq!(ORIGIN, "http://127.0.0.1:3098");
    let path = Path::new(&temp).join("d05-four-archived-chromium-private.json");
    let payload = json!({
        "old_session": old_session,
        "new_session": new_session,
        "chapters": [
            {"id": free[0], "kind": "free", "expected": FREE_ALPHA},
            {"id": free[1], "kind": "free", "expected": FREE_BETA},
            {"id": paid[0], "kind": "paid", "expected": "New V2, not purchased"},
            {"id": paid[1], "kind": "paid", "expected": "Second paid edition content"}
        ]
    });
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path).expect("private disposable fixture");
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("dedicated loopback Chromium server");
    let router = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("live Layer four-chapter Axum router");
    });
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/d05_four_archived_layer_live_chromium.cjs");
    assert!(script.is_file());
    let child_path = path.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "150s", "node"])
            .arg(script)
            .env("TOR_D05_FOUR_CHROMIUM_FIXTURE", child_path)
            .output()
    })
    .await
    .expect("Chromium runner");
    server.abort();
    let _ = server.await;
    fs::remove_file(&path).expect("delete fixture with private sessions");
    let output = output.expect("node, Linux timeout and Chromium required");
    assert!(
        output.status.success(),
        "unified D05 Chromium failed (exit {:?}); private diagnostics withheld",
        output.status.code()
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("D05 FOUR LIVE Chromium PASS"),
        "Chromium must actually read four chapters with one historical Layer"
    );
}
