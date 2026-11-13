//! REC-ADMIN-01: real Axum, PostgreSQL CAS and authorization for organic shelf ratios.
#![forbid(unsafe_code)]

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PAGE: &str = "/admin/recommendations";

async fn call(app: &Router, method: &str, cookie: &str, origin: &str, body: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(PAGE)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn form(csrf: &str, revision: i64, shares: [i32; 5], action: &str) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("csrf_token", csrf.to_owned()),
        ("expected_revision", revision.to_string()),
        ("action", action.to_owned()),
        ("author_pct", shares[0].to_string()),
        ("teammate_pct", shares[1].to_string()),
        ("other_authors_pct", shares[2].to_string()),
        ("no_team_author_pct", shares[3].to_string()),
        ("no_team_other_authors_pct", shares[4].to_string()),
    ]);
    url.query().unwrap().to_owned()
}

fn assert_private(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(response.headers()["x-robots-tag"]
        .to_str()
        .unwrap()
        .contains("noindex"));
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated disposable PostgreSQL 18 tor_ci with REC-POLICY migration"]
async fn admin_shares_http_requires_live_role_csrf_and_atomic_revision() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG required");
    assert!(url.starts_with("postgresql://tor_app:") && url.contains("@127.0.0.1:5432/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    // Share policy is one global singleton. Coordinate with REC fixture tests.
    let mut guard = pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(864863)")
        .execute(&mut *guard)
        .await
        .unwrap();

    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("RecommendationAdmin2026Fixture").unwrap();
    let mut users = Vec::new();
    let mut tokens = Vec::new();
    for is_admin in [true, false] {
        let token = generate_session_token();
        let id = accounts
            .register_with_session(
                &format!("rec-admin-{is_admin}-{stamp}@example.invalid"),
                "Recommendation admin fixture",
                &password,
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        if is_admin {
            sqlx::query("INSERT INTO account_roles(user_id,role) VALUES($1,'administrator')")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        users.push(id);
        tokens.push(token);
    }
    let csrf = generate_session_token();
    let admin_cookie = format!("tor-dev-session={}; tor-dev-csrf={csrf}", tokens[0]);
    let reader_cookie = format!("tor-dev-session={}", tokens[1]);
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.into(),
    );

    let initial: (i64, i16, i16, i16, i16, i16) = sqlx::query_as(
        "SELECT revision,author_pct,teammate_pct,other_authors_pct,
                no_team_author_pct,no_team_other_authors_pct
         FROM rec_shelf_share_policy WHERE singleton",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let previous = [
        i32::from(initial.1),
        i32::from(initial.2),
        i32::from(initial.3),
        i32::from(initial.4),
        i32::from(initial.5),
    ];
    let before_flow: i64 = sqlx::query_scalar("SELECT count(*) FROM flow_observation_proofs")
        .fetch_one(&pool)
        .await
        .unwrap();

    for (cookie, code) in [
        ("", StatusCode::UNAUTHORIZED),
        (reader_cookie.as_str(), StatusCode::FORBIDDEN),
    ] {
        for method in ["GET", "POST"] {
            let response = call(&app, method, cookie, ORIGIN, "").await;
            assert_eq!(response.status(), code);
            assert_private(&response);
        }
    }
    let response = call(&app, "GET", &admin_cookie, ORIGIN, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_private(&response);
    let html = body(response).await;
    assert!(html.contains(r#"data-testid="rec-shares-form""#));
    assert!(html.contains(&format!(r#"data-revision="{}""#, initial.0)));
    assert!(!html.contains("data-flow-proof"));

    let new_shares = [40, 30, 30, 60, 40];
    let good = form(&csrf, initial.0, new_shares, "save");
    let bad_csrf = form("wrong", initial.0, new_shares, "save");
    for (origin, data) in [
        (ORIGIN, bad_csrf.as_str()),
        ("https://evil.example", good.as_str()),
    ] {
        let response = call(&app, "POST", &admin_cookie, origin, data).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_private(&response);
    }
    let invalid_sum = form(&csrf, initial.0, [50, 40, 20, 70, 30], "save");
    assert_eq!(
        call(&app, "POST", &admin_cookie, ORIGIN, &invalid_sum)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let invalid_number = good.replace("author_pct=40", "author_pct=invalid");
    assert_eq!(
        call(&app, "POST", &admin_cookie, ORIGIN, &invalid_number)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let response = call(&app, "POST", &admin_cookie, ORIGIN, &good).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_private(&response);
    assert_eq!(response.headers()[header::LOCATION], PAGE);
    let after: (i64, i16, i16, i16, i16, i16) = sqlx::query_as(
        "SELECT revision,author_pct,teammate_pct,other_authors_pct,
                no_team_author_pct,no_team_other_authors_pct
         FROM rec_shelf_share_policy WHERE singleton",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after.0, initial.0 + 1);
    assert_eq!(
        [after.1, after.2, after.3, after.4, after.5],
        [40, 30, 30, 60, 40]
    );
    assert_eq!(
        call(&app, "POST", &admin_cookie, ORIGIN, &good)
            .await
            .status(),
        StatusCode::CONFLICT
    );

    // Return the singleton to its original values; test must not contaminate
    // subsequent REC fixtures, even when original differs from defaults.
    let restore = form(&csrf, after.0, previous, "save");
    assert_eq!(
        call(&app, "POST", &admin_cookie, ORIGIN, &restore)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let after_flow: i64 = sqlx::query_scalar("SELECT count(*) FROM flow_observation_proofs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before_flow, after_flow);

    sqlx::query("DELETE FROM account_roles WHERE user_id=$1 AND role='administrator'")
        .bind(users[0])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=ANY($1::bigint[])")
        .bind(&users)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_unlock(864863)")
        .execute(&mut *guard)
        .await
        .unwrap();
}
