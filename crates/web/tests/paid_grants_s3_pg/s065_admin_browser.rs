//! S065: real Axum/PG/MinIO and Chromium administrative reader, no purchase.
//! Only disposable tor_ci and loopback; session credentials are never logged.
use super::*;
use tor_accounts::services::security::session_token_digest;

// The same disposable PostgreSQL is reused by unrelated A-05 tests.
// A failed browser assertion must never leave a synthetic administrator active.
struct AdminRoleCleanup<'a> {
    url: &'a str,
    user_id: i64,
}
impl Drop for AdminRoleCleanup<'_> {
    fn drop(&mut self) {
        pg(
            self.url,
            &format!(
                "UPDATE account_roles SET is_active=FALSE \
                 WHERE user_id={} AND role='administrator'",
                self.user_id
            ),
        );
    }
}

pub(super) async fn verify(
    app: &Router,
    accounts: &PgAccounts,
    url: &str,
    chapter: i64,
    version: i64,
    outsider_session: &str,
) {
    assert!(
        url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"),
        "admin browser must use the disposable local database"
    );
    let (admin_id, admin_session) =
        user(accounts, "s065-admin-minio-chromium@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \
             VALUES({admin_id},'administrator',TRUE) \
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    let _cleanup = AdminRoleCleanup {
        url,
        user_id: admin_id,
    };
    let actor = accounts
        .session_actor(&session_token_digest(&admin_session))
        .await
        .unwrap()
        .expect("admin Chromium requires a real active role");
    assert!(actor.administrator);
    assert_eq!(actor.actor.user_id.get(), admin_id);
    assert_eq!(
        private_get(app, &format!("/read/admin/{chapter}"), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        private_get(
            app,
            &format!("/read/admin/{chapter}"),
            Some(outsider_session)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let temp = env::var("RUNNER_TEMP").expect("isolated CI runner directory");
    let fixture_path = Path::new(&temp).join("s065-admin-real-chromium.json");
    let payload = json!({
        "admin_id": admin_id,
        "admin_session": admin_session,
        "outsider_session": outsider_session,
        "chapter_id": chapter,
        "version_id": version,
        "asset_object_id": asset_fixture::ASSET_OBJECT_ID,
    });
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&fixture_path)
        .expect("private admin browser fixture");
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("exclusive S065 administrative Chromium listener");
    let http = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("S065 real HTTP server");
    });
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/s065_admin_real_chromium.cjs");
    assert!(
        script.is_file(),
        "S065 admin real browser runner must exist"
    );
    let child_fixture = fixture_path.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "180s", "node"])
            .arg(script)
            .env("TOR_S065_ADMIN_BROWSER_FIXTURE", child_fixture)
            .output()
    })
    .await
    .expect("S065 admin Chromium runner task");
    server.abort();
    let _ = server.await;
    std::fs::remove_file(&fixture_path).expect("remove ephemeral admin credentials");

    let output = output.expect("S065 requires Linux timeout and Chromium");
    assert!(
        output.status.success(),
        "S065 real admin Chromium failed (exit {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("S065 real admin Chromium PASS: desktop, mobile, revoke"),
        "both browsers must decrypt actual MinIO content before role revocation"
    );
    let current = accounts
        .session_actor(&session_token_digest(&admin_session))
        .await
        .unwrap()
        .expect("role revocation retains the original login");
    assert!(!current.administrator);
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={admin_id}")
        ),
        0,
        "admin inspection must not manufacture purchases"
    );
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM reader_wallet_entries WHERE user_id={admin_id}")
        ),
        0,
        "admin inspection must never debit a wallet"
    );
}
