//! D-04 / S034: real Axum sessions and disposable PostgreSQL checkout.
//! Never points to a working VPS, never activates a payment provider.
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
use tor_commerce::repositories::entitlements::PgDirectEntitlements;
use tor_contracts::paid_delivery::{
    Actor, DenialReason, EntitlementDecision, EntitlementPort, PositiveId,
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PATH: &str = "/api/v1/purchases";

async fn send(app: &Router, req: Request<Body>) -> Response {
    app.clone().oneshot(req).await.expect("Axum HTTP response")
}

fn get_me(session: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/me")
        .header(header::COOKIE, session)
        .body(Body::empty())
        .unwrap()
}

fn purchase(
    cookies: &str,
    origin: Option<&str>,
    csrf: Option<&str>,
    key: Option<&str>,
    body: &str,
) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(PATH)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookies);
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    request.body(Body::from(body.to_owned())).unwrap()
}

async fn body_json(reply: Response) -> Value {
    let bytes = to_bytes(reply.into_body(), 4096).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn cookie(response: &Response, name: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(name))
        .unwrap_or_else(|| panic!("missing cookie: {name}"))
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn actor(accounts: &PgAccounts, email: &str) -> (i64, String) {
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let hash = hash_password("D04StrongBuyerPassword2026").unwrap();
    let id = accounts
        .register_with_session(
            email,
            "D04 checkout",
            &hash,
            &digest,
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (id, format!("tor-dev-session={token}"))
}

async fn csrf(app: &Router, session: &str) -> (String, String) {
    let me = send(app, get_me(session)).await;
    assert_eq!(me.status(), StatusCode::OK);
    let csrf_cookie = cookie(&me, "tor-dev-csrf=");
    let csrf_value = csrf_cookie.split_once('=').unwrap().1.to_owned();
    (format!("{session}; {csrf_cookie}"), csrf_value)
}

async fn purchase_count(pool: &PgPool, buyer: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1")
        .bind(buyer)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "disposable TEST_DATABASE_URL=.../tor_ci; never run against a working server"]
async fn d_04_http_buyer_and_author_session_csrf_replay_and_isolation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());

    let (buyer, buyer_session) = actor(&accounts, "d04-buyer@tor-ci.example").await;
    let (author, author_session) = actor(&accounts, "d04-author@tor-ci.example").await;

    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,600,'RUB'),($2,600,'RUB')",
    )
    .bind(buyer)
    .bind(author)
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d04:purchase','ru','Проверка покупки','public',$1) RETURNING id",
    )
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Платная глава','public','paid',175) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'PAID-SECRET-D04-NOT-HTTP-READABLE') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();

    // This test is one isolated integration-test process: build a disabled
    // app first, then explicitly enable a second app in the same process.
    std::env::remove_var("TOR_PAID_CHECKOUT_ENABLED");
    let disabled = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let request = json!({
        "chapter_id": chapter.to_string(),
        "expected_version_id": version.to_string(),
        "expected_price_minor": 175
    })
    .to_string();
    let key = format!("{}8", "A".repeat(42));
    assert_eq!(
        send(&disabled, purchase(&buyer_session, Some(ORIGIN), None, Some(&key), &request))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "release builds must leave checkout disabled by default"
    );

    std::env::set_var("TOR_PAID_CHECKOUT_ENABLED", "1");
    let app = tor_web::router_with_postgres_reading_auth(
        PgCatalog::connect(&url).await.unwrap(),
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let (cookies, csrf_token) = csrf(&app, &buyer_session).await;
    let (author_cookies, author_csrf) = csrf(&app, &author_session).await;

    // No money operation without valid session, CSRF, origin and key.
    for (session, origin, csrf_value, request_key, expected) in [
        ("", Some(ORIGIN), Some(csrf_token.as_str()), Some(key.as_str()), StatusCode::UNAUTHORIZED),
        (cookies.as_str(), Some("https://evil.example"), Some(csrf_token.as_str()), Some(key.as_str()), StatusCode::FORBIDDEN),
        (cookies.as_str(), Some(ORIGIN), None, Some(key.as_str()), StatusCode::FORBIDDEN),
        (cookies.as_str(), Some(ORIGIN), Some("bad"), Some(key.as_str()), StatusCode::FORBIDDEN),
        (cookies.as_str(), Some(ORIGIN), Some(csrf_token.as_str()), None, StatusCode::UNPROCESSABLE_ENTITY),
        (cookies.as_str(), Some(ORIGIN), Some(csrf_token.as_str()), Some("bad-key"), StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        assert_eq!(
            send(&app, purchase(session, origin, csrf_value, request_key, &request))
                .await
                .status(),
            expected
        );
    }
    assert_eq!(purchase_count(&pool, buyer).await, 0);
    assert_eq!(
        send(
            &app,
            purchase(
                &cookies,
                Some(ORIGIN),
                Some(&csrf_token),
                Some(&key),
                &json!({
                    "chapter_id": chapter.to_string(),
                    "expected_version_id": version.to_string(),
                    "expected_price_minor": 175,
                    "user_id": author.to_string()
                })
                .to_string()
            )
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "request body cannot select an actor or wallet"
    );

    // The author cannot buy their own chapter, even with a valid session.
    assert_eq!(
        send(
            &app,
            purchase(
                &author_cookies,
                Some(ORIGIN),
                Some(&author_csrf),
                Some(&key),
                &request
            )
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(purchase_count(&pool, author).await, 0);

    // The real buyer purchases V1 once. Replay returns the original receipt.
    let accepted = send(
        &app,
        purchase(&cookies, Some(ORIGIN), Some(&csrf_token), Some(&key), &request),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(accepted.headers()[header::CACHE_CONTROL], "no-store");
    let receipt = body_json(accepted).await;
    assert_eq!(receipt["balance_minor"], 425);
    assert_eq!(receipt["chapter_id"], chapter.to_string());
    assert_eq!(receipt["version_id"], version.to_string());
    assert_eq!(receipt["replayed"], false);
    assert!(!receipt.to_string().contains("PAID-SECRET"));
    let original_id = receipt["purchase_id"].as_str().unwrap().to_owned();

    let replay = send(
        &app,
        purchase(&cookies, Some(ORIGIN), Some(&csrf_token), Some(&key), &request),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = body_json(replay).await;
    assert_eq!(replay["purchase_id"], original_id);
    assert_eq!(replay["balance_minor"], 425);
    assert_eq!(replay["replayed"], true);

    let changed_price = json!({
        "chapter_id": chapter.to_string(),
        "expected_version_id": version.to_string(),
        "expected_price_minor": 176
    })
    .to_string();
    assert_eq!(
        send(&app, purchase(&cookies, Some(ORIGIN), Some(&csrf_token), Some(&key), &changed_price))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let different_key = format!("{}8", "B".repeat(42));
    assert_eq!(
        send(
            &app,
            purchase(&cookies, Some(ORIGIN), Some(&csrf_token), Some(&different_key), &request)
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "second command key may not purchase an owned chapter twice"
    );

    // Stranger remains unentitled even after learning the buyer's purchase ID.
    let rights = PgDirectEntitlements::new(pool.clone());
    assert_eq!(
        rights
            .resolve_entitlement(
                Actor {
                    user_id: PositiveId::new(author).unwrap(),
                },
                PositiveId::new(chapter).unwrap(),
                Some(PositiveId::new(original_id.parse().unwrap()).unwrap()),
            )
            .await
            .unwrap(),
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        }
    );

    assert_eq!(purchase_count(&pool, buyer).await, 1);
    let debits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_wallet_entries
         WHERE user_id=$1 AND kind='chapter_purchase'",
    )
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(debits, 1);
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(balance, 425);

    std::env::remove_var("TOR_PAID_CHECKOUT_ENABLED");
}
