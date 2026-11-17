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
use sqlx::PgPool;
use std::num::NonZeroU32;
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

async fn book(pool: &PgPool, owner: i64, name: &str, tier: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','S020 age paid','public',$2,$3) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(tier)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn chapter(pool: &PgPool, book: i64, number: i32) -> (i64, i64) {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,'S020 chapter','public','paid',100) RETURNING id",
    )
    .bind(book)
    .bind(number)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'S020 PRIVATE BODY') RETURNING id",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(id)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
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

async fn finance(pool: &PgPool, buyer: i64) -> (i64, i64, i64, i64) {
    let wallet: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(pool)
            .await
            .unwrap();
    let purchases: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chapter_purchase_facts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(pool)
            .await
            .unwrap();
    let journal: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reader_wallet_entries WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(pool)
            .await
            .unwrap();
    let rights: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM library_purchased_chapters WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(pool)
            .await
            .unwrap();
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
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1000,'RUB')",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();

    let adult = book(
        &pool,
        owner,
        &format!("ci:s020:http:adult:{buyer}"),
        "adult",
    )
    .await;
    let a = chapter(&pool, adult, 1).await;
    let b = chapter(&pool, adult, 2).await;
    let c = chapter(&pool, adult, 3).await;
    let pro = book(
        &pool,
        owner,
        &format!("ci:s020:http:pro:{buyer}"),
        "pro_adult",
    )
    .await;
    let d = chapter(&pool, pro, 1).await;
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
    assert_eq!(finance(&pool, buyer).await, (1000, 0, 0, 0));

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
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
    assert_eq!(finance(&pool, buyer).await, (700, 3, 3, 3));

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    let replay = post(&app, &token, &csrf, 'A', "/api/v1/purchases", one).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await["replayed"], true);
    let replay_batch = post(&app, &token, &csrf, 'B', "/api/v1/purchases/batch", batch).await;
    assert_eq!(replay_batch.status(), StatusCode::OK);
    assert_eq!(json_body(replay_batch).await["replayed"], true);
    assert_eq!(
        post(&app, &token, &csrf, 'C', "/api/v1/purchases", pro_one.clone())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(finance(&pool, buyer).await, (700, 3, 3, 3));

    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let granted = post(&app, &token, &csrf, 'C', "/api/v1/purchases", pro_one.clone()).await;
    assert_eq!(granted.status(), StatusCode::OK);
    assert_eq!(json_body(granted).await["replayed"], false);
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    let historical = post(&app, &token, &csrf, 'C', "/api/v1/purchases", pro_one).await;
    assert_eq!(historical.status(), StatusCode::OK);
    assert_eq!(json_body(historical).await["replayed"], true);
    assert_eq!(finance(&pool, buyer).await, (600, 4, 4, 4));
    pool.close().await;
}
