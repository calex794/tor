//! P0 S020/D: live HTTP/PG paid checkout must agree with current Book age.
//! The financial API uses a real account session, Origin, CSRF and idempotency.
//! Never run against a real wallet, only disposable /tor_ci PostgreSQL.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use serde_json::{json, Value};
use std::{num::NonZeroU32, process::Command};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

async fn account(accounts: &PgAccounts, who: &str) -> (i64, String) {
    let token = generate_session_token();
    let email = format!(
        "s020-http-{who}-{}@example.test",
        token[..12].to_ascii_lowercase()
    );
    let user = accounts
        .register_with_session(
            &email,
            "S020 paid age",
            &hash_password("S020PaidAgeHttpStrongPassword2026").unwrap(),
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (user, token)
}

// Match the SQLx-free web crate boundary used by other real HTTP/PG tests.
fn pg(url: &str, query: &str) -> String {
    assert!(
        url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"),
        "only disposable tor_ci is permitted"
    );
    let output = Command::new("psql")
        .args([
            "-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1",
            "--dbname", url, "--command", query,
        ])
        .output()
        .expect("isolated PostgreSQL client required");
    assert!(
        output.status.success(),
        "SQL fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn scalar(url: &str, query: &str) -> i64 {
    pg(url, query).parse().expect("one integer PostgreSQL result")
}

fn book(url: &str, owner: i64, name: &str, tier: &str) -> i64 {
    assert!(matches!(tier, "basic" | "adult" | "pro_adult"));
    assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b":-_".contains(&b)));
    scalar(
        url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('{name}','ru','S020 age paid','public',{owner},'{tier}') RETURNING id"
        ),
    )
}

fn chapter(url: &str, book: i64, number: i32) -> (i64, i64) {
    let id = scalar(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},{number},'S020 chapter','public','paid',100) RETURNING id"
        ),
    );
    let version = scalar(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({id},1,'S020 PRIVATE BODY') RETURNING id"
        ),
    );
    pg(
        url,
        &format!("INSERT INTO chapter_publications(chapter_id,version_id) VALUES({id},{version})"),
    );
    (id, version)
}

async fn post(
    app: &Router,
    session: &str,
    csrf: &str,
    key: char,
    url: &str,
    body: Value,
) -> Response {
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let request = Request::builder()
        .method("POST")
        .uri(url)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header("x-csrf-token", csrf)
        .header(
            "idempotency-key",
            format!("{}8", key.to_string().repeat(42)),
        )
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn json_body(response: Response) -> Value {
    let raw = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
    serde_json::from_slice(&raw).unwrap()
}

fn finance(url: &str, buyer: i64) -> (i64, i64, i64, i64) {
    let wallet = scalar(
        url,
        &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}"),
    );
    let purchases = scalar(
        url,
        &format!("SELECT COUNT(*) FROM chapter_purchase_facts WHERE user_id={buyer}"),
    );
    let journal = scalar(
        url,
        &format!("SELECT COUNT(*) FROM reader_wallet_entries WHERE user_id={buyer}"),
    );
    let rights = scalar(
        url,
        &format!("SELECT COUNT(*) FROM library_purchased_chapters WHERE user_id={buyer}"),
    );
    (wallet, purchases, journal, rights)
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and TOR_PAID_CHECKOUT_ENABLED=1"]
async fn s020_age_checkout_http_denies_and_restores_single_batch_without_false_debits() {
    assert_eq!(
        std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated database");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));

    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let reader = PgReader::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, _) = account(&accounts, "owner").await;
    let (buyer, token) = account(&accounts, "buyer").await;
    let csrf = generate_session_token();
    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({buyer},1000,'RUB')"
        ),
    );

    let adult = book(
        &url,
        owner,
        &format!("ci:s020:http:adult:{buyer}"),
        "adult",
    );
    let a = chapter(&url, adult, 1);
    let b = chapter(&url, adult, 2);
    let c = chapter(&url, adult, 3);
    let pro = book(
        &url,
        owner,
        &format!("ci:s020:http:pro:{buyer}"),
        "pro_adult",
    );
    let d = chapter(&url, pro, 1);
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let one = json!({
        "chapter_id": a.0.to_string(),
        "expected_version_id": a.1.to_string(),
        "expected_price_minor": 100
    });
    let batch = json!({"chapters": [
        {"chapter_id": b.0.to_string(), "expected_version_id": b.1.to_string(), "expected_price_minor": 100},
        {"chapter_id": c.0.to_string(), "expected_version_id": c.1.to_string(), "expected_price_minor": 100}
    ]});
    let pro_one = json!({
        "chapter_id": d.0.to_string(),
        "expected_version_id": d.1.to_string(),
        "expected_price_minor": 100
    });
    assert_eq!(
        post(&app, &token, &csrf, 'A', "/api/v1/purchases", one.clone())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(
            &app,
            &token,
            &csrf,
            'B',
            "/api/v1/purchases/batch",
            batch.clone()
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(finance(&url, buyer), (1000, 0, 0, 0));

    pg(&url, &format!("UPDATE users SET adult_enabled=TRUE WHERE id={buyer}"));
    let single = post(&app, &token, &csrf, 'A', "/api/v1/purchases", one.clone()).await;
    assert_eq!(single.status(), StatusCode::OK);
    assert_eq!(json_body(single).await["replayed"], false);
    let several = post(
        &app,
        &token,
        &csrf,
        'B',
        "/api/v1/purchases/batch",
        batch.clone(),
    )
    .await;
    assert_eq!(several.status(), StatusCode::OK);
    assert_eq!(
        json_body(several).await["purchases"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(finance(&url, buyer), (700, 3, 3, 3));

    pg(&url, &format!("UPDATE users SET adult_enabled=FALSE WHERE id={buyer}"));
    let replay = post(&app, &token, &csrf, 'A', "/api/v1/purchases", one).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await["replayed"], true);
    let replay_batch = post(&app, &token, &csrf, 'B', "/api/v1/purchases/batch", batch).await;
    assert_eq!(replay_batch.status(), StatusCode::OK);
    assert_eq!(json_body(replay_batch).await["replayed"], true);
    assert_eq!(
        post(
            &app,
            &token,
            &csrf,
            'C',
            "/api/v1/purchases",
            pro_one.clone()
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(finance(&url, buyer), (700, 3, 3, 3));

    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({buyer},'pro_adult','moderation',TRUE)"
        ),
    );
    let granted = post(
        &app,
        &token,
        &csrf,
        'C',
        "/api/v1/purchases",
        pro_one.clone(),
    )
    .await;
    assert_eq!(granted.status(), StatusCode::OK);
    assert_eq!(json_body(granted).await["replayed"], false);
    pg(
        &url,
        &format!("UPDATE user_level_grants SET is_active=FALSE WHERE user_id={buyer}"),
    );
    let historical = post(&app, &token, &csrf, 'C', "/api/v1/purchases", pro_one).await;
    assert_eq!(historical.status(), StatusCode::OK);
    assert_eq!(json_body(historical).await["replayed"], true);
    assert_eq!(finance(&url, buyer), (600, 4, 4, 4));
    pool.close().await;
}
