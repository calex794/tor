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
    url: &str,
    sessions: (&str, &str, &str, &str),
    free: [i64; 2],
    paid: [i64; 2],
    term_id: i64,
) {
    let (old_session, new_session, expired_session, outsider_session) = sessions;
    assert!(term_id > 0);
    let temp = env::var("RUNNER_TEMP").expect("disposable CI temp directory");
    let configured_url = env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(configured_url.ends_with("/tor_ci") && configured_url.contains("@127.0.0.1:5432/"));
    assert_eq!(
        configured_url, url,
        "browser must use the same disposable PG connection"
    );
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    assert_eq!(ORIGIN, "http://127.0.0.1:3098");
    let path = Path::new(&temp).join("d05-four-archived-chromium-private.json");
    let payload = json!({
        "old_session": old_session,
        "new_session": new_session,
        "expired_session": expired_session,
        "outsider_session": outsider_session,
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
    // The two phases use the same actor-scoped Rust router, actual PG18 and
    // MinIO. No mock HTTP handlers or synthetic grants are introduced.
    let launch = |phase: &'static str| {
        let script = script.clone();
        let child_path = path.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                Command::new("timeout")
                    .args(["-k", "5s", "150s", "node"])
                    .arg(script)
                    .env("TOR_D05_FOUR_CHROMIUM_FIXTURE", child_path)
                    .env("TOR_D05_FOUR_CHROMIUM_PHASE", phase)
                    .output()
            })
            .await
            .expect("Chromium runner")
            .expect("node, Linux timeout and Chromium required")
        }
    };
    let initial = launch("initial").await;
    let opened = initial.status.success()
        && String::from_utf8_lossy(&initial.stdout).contains("D05 FOUR LIVE Chromium PASS");
    let revoked = if opened {
        // This is the same term that successfully unlocked all four chapters.
        // Legal revocation must cut off both FREE and PAID immediately, with
        // no refund, new direct purchase, or mutation of the original term.
        super::pg(
            url,
            &format!(
                "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
                 VALUES({term_id},'legal_block','ci:d05-four-chapter-revoked:{term_id}')"
            ),
        );
        Some(launch("revoked").await)
    } else {
        None
    };
    server.abort();
    let _ = server.await;
    fs::remove_file(&path).expect("delete fixture with private sessions");
    assert!(
        opened,
        "unified D05 Chromium could not read four chapters; confidential diagnostics withheld"
    );
    let revoked = revoked.expect("positive browser phase must pass before legal revocation");
    assert!(
        revoked.status.success()
            && String::from_utf8_lossy(&revoked.stdout).contains("D05 FOUR REVOKED Chromium PASS"),
        "revoked term must deny all four private browsers"
    );
}
