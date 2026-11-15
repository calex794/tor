//! H/UI-20: real Axum/PG package navigation without any wallet debit.
//! Requires disposable tor_ci and explicitly enabled Layer storefront.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{num::NonZeroU32, process::Command};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_commerce::repositories::{layer_package_sales::PackagePaidMode, layers::PgLayers};
use tor_contracts::paid_delivery::PositiveId;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

fn id(raw: i64) -> PositiveId {
    PositiveId::new(raw).unwrap()
}

fn pg(url: &str, sql: &str) -> String {
    assert!(
        url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"),
        "H/UI-20 tests may only change disposable tor_ci"
    );
    let run = Command::new("psql")
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
            sql,
        ])
        .output()
        .expect("psql required");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8(run.stdout).unwrap().trim().to_owned()
}

fn number(url: &str, sql: &str) -> i64 {
    pg(url, sql).parse().unwrap()
}

async fn user(accounts: &PgAccounts, role: &str) -> (i64, String) {
    let token = generate_session_token();
    let email = format!(
        "h20-{role}-{}@example.test",
        token[..12].to_ascii_lowercase()
    );
    let actor = accounts
        .register_with_session(
            &email,
            "H20 tester",
            &hash_password("H20StrongPasswordForIsolatedTests2026").unwrap(),
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (actor, token)
}

async fn get(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().uri(path);
    if let Some(token) = token {
        request = request.header(header::COOKIE, format!("tor-dev-session={token}"));
    }
    let result = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = result.status();
    let html = String::from_utf8(
        to_bytes(result.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, html)
}

fn create_attached_paid_chapter(url: &str, book: i64) -> i64 {
    let chapter = number(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'H20 paid','public','paid',75) RETURNING id"
        ),
    );
    let version = number(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'H20 sealed fixture') RETURNING id"
        ),
    );
    let object_id = pg(url, "SELECT gen_random_uuid()::text");
    pg(
        url,
        &format!(
            "INSERT INTO storage_objects
               (id,object_kind,bucket,object_key,state,size_bytes,
                sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
                kek_id,object_nonce,object_aad,codec,verified_at,attached_at)
             VALUES (
               '{object_id}'::UUID,'chapter','tor-ci-private',
               'private/chapter-versions/{object_id}.enc',
               'attached',42,
               decode(repeat('11',32),'hex'),
               sha256(convert_to('H20 sealed fixture','UTF8')),
               decode(repeat('22',48),'hex'),
               decode(repeat('33',12),'hex'),
               'content-kek-v1',
               decode(repeat('44',12),'hex'),
               convert_to('tor.encrypted.chapter.v1' || chr(10) ||
                 '{chapter}' || chr(10) || '{version}' || chr(10) ||
                 '{object_id}' || chr(10) ||
                 encode(sha256(convert_to('H20 sealed fixture','UTF8')),'hex') || chr(10),
                 'UTF8'),
               'gzip',now(),now()
             ) RETURNING id"
        ),
    );
    pg(
        url,
        &format!(
            "UPDATE chapter_versions SET storage_object_id='{object_id}'::UUID
             WHERE id={version} RETURNING id"
        ),
    );
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{version}) RETURNING chapter_id"
        ),
    );
    chapter
}

async fn chromium_public_package(app: &Router, book: i64, layer: i64) {
    let temporary = std::env::var("RUNNER_TEMP").expect("isolated browser test directory");
    let fixture_path = std::path::Path::new(&temporary).join("h20-public-package.json");
    let fixture = serde_json::json!({"book": book, "layer": layer});
    std::fs::write(&fixture_path, fixture.to_string()).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("isolated loopback HTTP port");
    let router = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("live package HTTP server");
    });
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/h_ui_20_package_browser.cjs");
    assert!(script.is_file());
    let output = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["-k", "5s", "120s", "node"])
            .arg(script)
            .env("TOR_H20_REAL_BROWSER_FIXTURE", fixture_path)
            .output()
    })
    .await
    .unwrap()
    .expect("Node and Chromium required");
    server.abort();
    let _ = server.await;
    std::fs::remove_file(std::path::Path::new(&temporary).join("h20-public-package.json"))
        .unwrap();
    assert!(
        output.status.success(),
        "H/UI-20 Chromium failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("H/UI-20 Chromium PASS: desktop and mobile")
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and explicitly enabled package checkout"]
async fn h_ui_20_book_and_batch_http_link_only_to_current_public_package() {
    for name in [
        "TOR_PAID_CHECKOUT_ENABLED",
        "TOR_LAYER_PAGES_ENABLED",
        "TOR_LAYER_CHECKOUT_ENABLED",
    ] {
        assert_eq!(std::env::var(name).as_deref(), Ok("1"));
    }
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let reader = PgReader::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, _) = user(&accounts, "owner").await;
    let (buyer, token) = user(&accounts, "reader").await;
    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({buyer},500,'RUB') RETURNING user_id"
        ),
    );
    let book = number(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:h20:http:{owner}','ru','H20 <script>unsafe</script>','public',{owner},'basic')
             RETURNING id"
        ),
    );
    let chapter = create_attached_paid_chapter(&url, book);
    let layers = PgLayers::new(pool.clone());
    let layer = layers
        .create_layer(id(owner), "H20 <b>пакет</b>")
        .await
        .unwrap();
    let version = layers
        .add_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    layers
        .publish_offer(id(owner), id(layer), 125, false)
        .await
        .unwrap();
    layers
        .set_package_paid_mode(
            id(owner),
            id(layer),
            id(book),
            id(version),
            0,
            PackagePaidMode::PackageOnly,
        )
        .await
        .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        "http://127.0.0.1:3098".to_owned(),
    );
    let (status, card) = get(&app, &format!("/books/{book}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(card.contains("Доступно через пакет"), "{card}");
    assert!(card.contains(&format!("href=\"/layers/{layer}\"")));
    assert!(card.contains("H20 &lt;b&gt;пакет&lt;/b&gt;"));
    assert!(!card.contains("<script>unsafe</script>"));
    assert!(!card.contains("H20 sealed fixture"));

    let (status, batch) = get(
        &app,
        &format!("/me/books/{book}/buy-chapters"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(batch.contains("Доступно через пакет"));
    assert!(batch.contains(&format!("href=\"/layers/{layer}\"")));
    assert!(
        !batch.contains("name=\"chapter_"),
        "do not invite a blocked direct purchase"
    );
    let (status, storefront) = get(&app, &format!("/layers/{layer}"), Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(storefront.contains("H20 &lt;b&gt;пакет&lt;/b&gt;"));
    assert!(!storefront.contains("H20 sealed fixture"));
    assert_eq!(
        number(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
        ),
        500,
    );
    assert_eq!(
        number(
            &url,
            &format!("SELECT COUNT(*) FROM chapter_purchase_facts WHERE user_id={buyer}")
        ),
        0,
        "the navigation must not create purchases",
    );

    chromium_public_package(&app, book, layer).await;

    // Hiding the active package removes every public purchase link.
    pg(
        &url,
        &format!("UPDATE commerce_layers SET status='hidden' WHERE id={layer} RETURNING id"),
    );
    let (status, card) = get(&app, &format!("/books/{book}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!card.contains(&format!("href=\"/layers/{layer}\"")));
    let (status, _) = get(&app, &format!("/layers/{layer}"), Some(&token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let _ = chapter;
    pool.close().await;
}
