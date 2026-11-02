//! S034 acceptance: a real Chromium context confirms one chapter purchase,
//! reads its private S3 edition, then loses access after canonical revocation.
//! Only disposable tor_ci, loopback, test sessions and ephemeral CI fixtures.
use super::*;

pub(super) struct Fixture<'a> {
    pub book: i64,
    pub chapters: [(i64, i64); 2],
    pub buyers: [(i64, &'a str); 2],
    pub outsider_session: &'a str,
}

pub(super) async fn verify(app: &Router, fixture: Fixture<'_>) {
    assert_eq!(env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(), Ok("1"));
    let temp = env::var("RUNNER_TEMP").expect("isolated CI temp directory");
    let fixture_path = Path::new(&temp).join("s034-contiguous-private-reader.json");
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
        "outsider_session": fixture.outsider_session,
    });
    // No cookies or bearer credentials in source control, artifacts or logs.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&fixture_path)
        .expect("S034 ephemeral fixture must be a new file");
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);

    // Unlike the separate reader JavaScript fixture, this is the LIVE Rust
    // Axum HTTP router: single checkout, grants and the paid reader.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("S034 loopback port reserved for this CI test");
    let http = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("S034 real HTTP server");
    });
    let child_fixture = fixture_path.clone();
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/s034_single_paid_browser.cjs");
    assert!(script.is_file(), "S034 real Chromium script must exist");
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "180s", "node"])
            .arg(script)
            .env("TOR_S034_REAL_BROWSER_FIXTURE", child_fixture)
            .output()
    })
    .await
    .expect("S034 Chromium runner thread");
    server.abort();
    let _ = server.await;
    std::fs::remove_file(&fixture_path).expect("remove temporary session fixture");

    let output = output.expect("S034 requires Linux timeout and Node/Chromium");
    assert!(
        output.status.success(),
        "S034 real buy-to-reader Chromium failed (exit {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("S034 single Chromium PASS: desktop")
            && stdout.contains("S034 single Chromium PASS: mobile"),
        "both viewport acceptance runs must buy and then lose access after revocation"
    );
}

pub(super) async fn verify_new_buyers(
    app: &Router,
    accounts: &PgAccounts,
    url: &str,
    book: i64,
    chapters: [(i64, i64); 2],
) {
    let (desktop, desktop_session) = user(accounts, "s034-single-desktop@tor-ci.example").await;
    let (mobile, mobile_session) = user(accounts, "s034-single-mobile@tor-ci.example").await;
    let (_, outsider_session) = user(accounts, "s065-outsider@tor-ci.example").await;
    pg(url, &format!("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES({desktop},600,'RUB'),({mobile},600,'RUB')"));
    verify(
        app,
        Fixture {
            book,
            chapters,
            buyers: [(desktop, &desktop_session), (mobile, &mobile_session)],
            outsider_session: &outsider_session,
        },
    )
    .await;
}
