//! S035 acceptance: the SAME real Chromium context buys a batch then reads
//! both separate encrypted editions through the actual Rust router and MinIO.
//! Only disposable tor_ci, loopback, test sessions and ephemeral CI fixtures.
use super::*;

pub(super) struct Fixture<'a> {
    pub book: i64,
    pub chapters: [(i64, i64); 2],
    pub buyers: [(i64, &'a str); 2],
}

pub(super) async fn verify(app: &Router, fixture: Fixture<'_>) {
    assert_eq!(env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(), Ok("1"));
    let temp = env::var("RUNNER_TEMP").expect("isolated CI temp directory");
    let fixture_path = Path::new(&temp).join("s035-contiguous-private-reader.json");
    let chapter_rows: Vec<Value> = fixture
        .chapters
        .iter()
        .map(|(id, version)| json!({"id": id, "version": version}))
        .collect();
    let buyer_rows: Vec<Value> = fixture
        .buyers
        .iter()
        .map(|(id, session)| json!({"id": id, "session": session}))
        .collect();
    let payload = json!({
        "book_id": fixture.book,
        "chapters": chapter_rows,
        "buyers": buyer_rows,
    });
    // No cookies or bearer credentials in source control, artifacts or logs.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&fixture_path)
        .expect("S035 ephemeral fixture must be a new file");
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);

    // Unlike the separate reader JavaScript fixture, this is the LIVE Rust
    // Axum HTTP router: selection, checkout, grants and both paid pages.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("S035 loopback port reserved for this CI test");
    let http = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, http)
            .await
            .expect("S035 real HTTP server");
    });
    let child_fixture = fixture_path.clone();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/s035_contiguous_paid_browser.cjs");
    assert!(script.is_file(), "S035 real Chromium script must exist");
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "180s", "node"])
            .arg(script)
            .env("TOR_S035_REAL_BROWSER_FIXTURE", child_fixture)
            .output()
    })
    .await
    .expect("S035 Chromium runner thread");
    server.abort();
    let _ = server.await;
    std::fs::remove_file(&fixture_path).expect("remove temporary session fixture");

    let output = output.expect("S035 requires Linux timeout and Node/Chromium");
    assert!(
        output.status.success(),
        "S035 real buy-to-reader Chromium failed (exit {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("S035 contiguous Chromium PASS: desktop")
            && stdout.contains("S035 contiguous Chromium PASS: mobile"),
        "both viewport acceptance runs must reach the second decrypted reader"
    );
}
