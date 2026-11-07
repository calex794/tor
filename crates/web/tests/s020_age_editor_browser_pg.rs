//! S020/#486: real clickable Book age editor on disposable PostgreSQL/Chromium.
//! No server feature is enabled by default; only this in-process router opts in.
#![forbid(unsafe_code)]
use std::{
    num::NonZeroU32,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, query: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:"));
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    let result = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            query,
        ])
        .output()
        .expect("isolated PG client");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable /tor_ci, Node Playwright and Chromium"]
async fn s020_owner_age_editor_real_chromium_mobile_and_desktop() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.starts_with("postgresql://tor_app:"));
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let owner_token = generate_session_token();
    let reader_token = generate_session_token();
    let csrf = generate_session_token();
    let hash = hash_password("S020ChromiumAgeEditor2026!").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("s020-editor-owner-{stamp}@example.invalid"),
            "S020 browser owner",
            &hash,
            &session_token_digest(&owner_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let viewer = accounts
        .register_with_session(
            &format!("s020-editor-reader-{stamp}@example.invalid"),
            "S020 browser viewer",
            &hash,
            &session_token_digest(&reader_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,owner_user_id,visibility,access_level)
             VALUES('ci:s020:chromium:book:{stamp}','ru','S020 Owner Age UI',{owner},'public','basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'S020 reader V1','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let edition: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'S020-CHROMIUM-FREE-TEXT-{book}') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{edition})"
        ),
    );
    pg(
        &url,
        &format!("UPDATE users SET adult_enabled=TRUE WHERE id={viewer}"),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({owner},'pro_adult','moderation',TRUE)"
        ),
    );

    // Flag affects only this disposable Axum router; always reset process env.
    std::env::set_var("TOR_S020_ACCESS_EDITOR_ENABLED", "1");
    let site = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    std::env::remove_var("TOR_S020_ACCESS_EDITOR_ENABLED");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            site.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let result = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["--signal=TERM", "--kill-after=5s", "120s", "node"])
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tools/s020_age_editor_chromium.cjs"
            ))
            .env("TEST_DATABASE_URL", &url)
            .env("S020_BROWSER_BOOK_ID", book.to_string())
            .env("S020_BROWSER_FREE_ID", chapter.to_string())
            .env("S020_BROWSER_READER_ID", viewer.to_string())
            .env("S020_BROWSER_OWNER_SESSION", owner_token)
            .env("S020_BROWSER_READER_SESSION", reader_token)
            .env("S020_BROWSER_CSRF", csrf)
            .output()
            .expect("Node Playwright and Chromium available in PG CI")
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
    assert!(
        result.status.success(),
        "S020 Chromium failed ({}): stdout={}, stderr={}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout)
        .contains("S020 owner age editor Chromium PASS: desktop, mobile, grant, revoke"));
    // Persisted facts: no refund, purchase or wallet row was constructed.
    assert_eq!(
        pg(
            &std::env::var("TEST_DATABASE_URL").unwrap(),
            &format!("SELECT access_level FROM books WHERE id={book}")
        ),
        "pro_adult"
    );
    pool.close().await;
}
