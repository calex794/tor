//! D-05 / S031: real authenticated HTTP Layer checkout against isolated PostgreSQL.
//! No external payment provider, and never run against a production database.
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
use tor_commerce::repositories::layers::PgLayers;
use tor_contracts::paid_delivery::PositiveId;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn id(value: i64) -> PositiveId {
    PositiveId::new(value).unwrap()
}

async fn account(accounts: &PgAccounts, role: &str) -> (i64, String) {
    let token = generate_session_token();
    let password = hash_password("D05LayerHttpTestStrongPassword2026").unwrap();
    let email = format!("d05-http-{role}-{}@example.test", &token[..12]);
    let id = accounts
        .register_with_session(
            &email,
            "D05 test",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (id, token)
}

async fn post(
    app: &Router,
    path: &str,
    body: &str,
    (session, csrf_cookie, csrf_header, origin, key): (
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
    ),
) -> Response {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(session) = session {
        let cookie = match csrf_cookie {
            Some(csrf) => format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
            None => format!("tor-dev-session={session}"),
        };
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf_header {
        request = request.header("x-csrf-token", csrf);
    }
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn financial_state(pool: &PgPool, buyer: i64) -> (i64, i64, i64) {
    let balance =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(pool)
            .await
            .unwrap();
    let terms = sqlx::query_scalar("SELECT COUNT(*) FROM commerce_layer_terms WHERE user_id=$1")
        .bind(buyer)
        .fetch_one(pool)
        .await
        .unwrap();
    let entries = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reader_wallet_entries WHERE user_id=$1 AND kind='layer_purchase'",
    )
    .bind(buyer)
    .fetch_one(pool)
    .await
    .unwrap();
    (balance, terms, entries)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci with TOR_PAID_CHECKOUT_ENABLED=1"]
async fn d_05_real_layer_http_pg_session_csrf_replay_and_wallet() {
    assert_eq!(
        std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(),
        Ok("1"),
        "explicit opt-in required to exercise the real checkout router"
    );
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated TEST_DATABASE_URL");
    assert!(
        url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"),
        "never run financial HTTP tests on production or as a migrator"
    );
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let reader = PgReader::new(pool.clone());
    reader.probe().await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    accounts.probe().await.unwrap();

    let (owner, _) = account(&accounts, "author").await;
    let (buyer, buyer_token) = account(&accounts, "buyer").await;
    let (poor, poor_token) = account(&accounts, "low-balance").await;
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,300,'RUB'),($2,20,'RUB')",
    )
    .bind(buyer)
    .bind(poor)
    .execute(&pool)
    .await
    .unwrap();

    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','D05 HTTP book','public',$2) RETURNING id",
    )
    .bind(format!("ci:d05:http:{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let layers = PgLayers::new(pool.clone());
    let layer = layers
        .create_layer(id(owner), "D05 HTTP package")
        .await
        .unwrap();
    let version = layers
        .add_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    let offer = layers
        .publish_offer(id(owner), id(layer), 90, false)
        .await
        .unwrap();

    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let path = format!("/api/v1/layers/{layer}/purchase");
    let valid = json!({
        "expected_offer_id": offer.to_string(),
        "expected_version_id": version.to_string(),
        "expected_price_minor": 90,
    })
    .to_string();
    let csrf = generate_session_token();
    let key = format!("{}8", "A".repeat(42));
    let other_key = format!("{}8", "B".repeat(42));

    assert_eq!(
        post(
            &app,
            &path,
            &valid,
            (None, None, None, Some(ORIGIN), Some(&key))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    for (cookie, posted, origin, idem, expected) in [
        (
            Some(csrf.as_str()),
            Some(csrf.as_str()),
            None,
            Some(key.as_str()),
            StatusCode::FORBIDDEN,
        ),
        (
            Some(csrf.as_str()),
            Some(csrf.as_str()),
            Some("https://evil.example"),
            Some(key.as_str()),
            StatusCode::FORBIDDEN,
        ),
        (
            Some(csrf.as_str()),
            None,
            Some(ORIGIN),
            Some(key.as_str()),
            StatusCode::FORBIDDEN,
        ),
        (
            None,
            Some(csrf.as_str()),
            Some(ORIGIN),
            Some(key.as_str()),
            StatusCode::FORBIDDEN,
        ),
        (
            Some(csrf.as_str()),
            Some(csrf.as_str()),
            Some(ORIGIN),
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            Some(csrf.as_str()),
            Some(csrf.as_str()),
            Some(ORIGIN),
            Some("invalid"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(
            post(
                &app,
                &path,
                &valid,
                (Some(&buyer_token), cookie, posted, origin, idem)
            )
            .await
            .status(),
            expected
        );
    }
    let tampered = json!({
        "expected_offer_id": offer.to_string(),
        "expected_version_id": version.to_string(),
        "expected_price_minor": 90,
        "user_id": poor.to_string(),
    })
    .to_string();
    assert_eq!(
        post(
            &app,
            &path,
            &tampered,
            (
                Some(&buyer_token),
                Some(&csrf),
                Some(&csrf),
                Some(ORIGIN),
                Some(&key)
            )
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(financial_state(&pool, buyer).await, (300, 0, 0));

    let poor_csrf = generate_session_token();
    assert_eq!(
        post(
            &app,
            &path,
            &valid,
            (
                Some(&poor_token),
                Some(&poor_csrf),
                Some(&poor_csrf),
                Some(ORIGIN),
                Some(&other_key)
            )
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "insufficient balance must fail without a paid term"
    );
    assert_eq!(financial_state(&pool, poor).await, (20, 0, 0));

    let wrong_price = json!({
        "expected_offer_id": offer.to_string(),
        "expected_version_id": version.to_string(),
        "expected_price_minor": 91,
    })
    .to_string();
    assert_eq!(
        post(
            &app,
            &path,
            &wrong_price,
            (
                Some(&buyer_token),
                Some(&csrf),
                Some(&csrf),
                Some(ORIGIN),
                Some(&other_key)
            )
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "a changed price is never silently accepted"
    );
    assert_eq!(financial_state(&pool, buyer).await, (300, 0, 0));

    // Two concurrent identical HTTP requests must commit exactly one wallet debit.
    let (first, second) = tokio::join!(
        post(
            &app,
            &path,
            &valid,
            (
                Some(&buyer_token),
                Some(&csrf),
                Some(&csrf),
                Some(ORIGIN),
                Some(&key)
            )
        ),
        post(
            &app,
            &path,
            &valid,
            (
                Some(&buyer_token),
                Some(&csrf),
                Some(&csrf),
                Some(ORIGIN),
                Some(&key)
            )
        ),
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(first.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(second.headers()[header::CACHE_CONTROL], "no-store");
    let a = json_body(first).await;
    let b = json_body(second).await;
    assert_eq!(a["term_id"], b["term_id"]);
    assert_eq!(a["balance_minor"], 210);
    assert_eq!(b["balance_minor"], 210);
    assert_ne!(a["replayed"], b["replayed"]);
    assert_eq!(financial_state(&pool, buyer).await, (210, 1, 1));

    assert_eq!(
        post(
            &app,
            &path,
            &wrong_price,
            (
                Some(&buyer_token),
                Some(&csrf),
                Some(&csrf),
                Some(ORIGIN),
                Some(&key)
            )
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "same key with changed purchase intent must not consume money"
    );
    assert_eq!(financial_state(&pool, buyer).await, (210, 1, 1));

    // A fresh key purchases one additional prepaid period, not a duplicate charge.
    let renewed = post(
        &app,
        &path,
        &valid,
        (
            Some(&buyer_token),
            Some(&csrf),
            Some(&csrf),
            Some(ORIGIN),
            Some(&other_key),
        ),
    )
    .await;
    assert_eq!(renewed.status(), StatusCode::OK);
    let renewal = json_body(renewed).await;
    assert_ne!(renewal["term_id"], a["term_id"]);
    assert_eq!(renewal["balance_minor"], 120);
    assert_eq!(financial_state(&pool, buyer).await, (120, 2, 2));
    pool.close().await;
}
