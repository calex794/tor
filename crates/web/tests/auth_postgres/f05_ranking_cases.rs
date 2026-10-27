//! F-05 real Axum session and PostgreSQL integration; SQL stays in F/A.
use super::*;
use std::num::NonZeroU32;
use tor_accounts::services::security::{
    generate_session_token, hash_password, session_token_digest,
};
use tor_recommendations::{
    repositories::{PgRanking, RankingCommand},
    services::ranking::RankingSnapshot,
};

async fn fixture(name: &str) -> (Router, PgRanking, i64, String) {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG URL");
    assert!(url.ends_with("/tor_ci"), "never use VPS");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let repo = PgRanking::new(catalog.pool().clone());
    let reader = PgReader::new(catalog.pool().clone());
    let email = format!("f05-api-{name}@tor-ci.example");
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let password = hash_password("F05TestLongStrongPassword2026").unwrap();
    let uid = accounts
        .register_with_session(
            &email,
            "F05 API",
            &password,
            &digest,
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    (app, repo, uid, format!("tor-dev-session={token}"))
}

async fn ready(repo: &PgRanking, uid: i64, books: [i64; 2]) -> RankingSnapshot {
    // Server-side test-only fixture. HTTP has no start route.
    let first = repo
        .apply(uid, 0, RankingCommand::StartVerified(books[0]))
        .await
        .unwrap();
    repo.apply(
        uid,
        first.snapshot.revision,
        RankingCommand::StartVerified(books[1]),
    )
    .await
    .unwrap()
    .snapshot
}

async fn csrf(app: &Router, session: &str) -> (String, String) {
    let page = send(app, request("GET", "/me/rankings", "", session, ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let value = cookie(&page, "tor-dev-csrf=");
    assert!(body_of(page).await.contains("Мой рейтинг"));
    let token = value.split_once('=').unwrap().1.to_owned();
    (value, token)
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_05_http_private_csrf_and_duplicate_fencing() {
    let (app, repo, uid, session) = fixture("choice").await;
    let guest = send(&app, request("GET", "/api/v1/me/rankings", "", "", ORIGIN)).await;
    assert_eq!(guest.status(), StatusCode::UNAUTHORIZED);
    let empty = send(
        &app,
        request("GET", "/api/v1/me/rankings", "", &session, ORIGIN),
    )
    .await;
    assert!(body_of(empty)
        .await
        .contains("awaiting_verified_completion"));
    // CI owns public books 1 and 901; no fabricated public content.
    let before = ready(&repo, uid, [1, 901]).await;
    let q = before.next_question().unwrap().unwrap();
    let (csrf_cookie, token) = csrf(&app, &session).await;
    let cookies = format!("{session}; {csrf_cookie}");
    let read = send(
        &app,
        request("GET", "/api/v1/me/rankings", "", &cookies, ORIGIN),
    )
    .await;
    assert_eq!(read.status(), StatusCode::OK);
    let data: serde_json::Value = serde_json::from_str(&body_of(read).await).unwrap();
    assert_eq!(data["state"], "choose");
    assert_eq!(data["question"]["comparison_id"], q.comparison_id);
    let json = format!(
        r#"{{"expected_revision":{},"expected_state_version":{},"comparison_id":"{}","outcome":"tie"}}"#,
        before.revision, q.state_version, q.comparison_id,
    );
    for (origin, csrf) in [
        (ORIGIN, "bad-token"),
        ("https://evil.example", token.as_str()),
    ] {
        let denied = send(
            &app,
            json_request(
                "POST",
                "/api/v1/me/rankings/choice",
                &json,
                &cookies,
                origin,
                csrf,
            ),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    let accepted = send(
        &app,
        json_request(
            "POST",
            "/api/v1/me/rankings/choice",
            &json,
            &cookies,
            ORIGIN,
            &token,
        ),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert!(body_of(accepted).await.contains(r#""status":"saved""#));
    let ranked = send(
        &app,
        request("GET", "/api/v1/me/rankings", "", &cookies, ORIGIN),
    )
    .await;
    assert_eq!(ranked.status(), StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body_of(ranked).await).unwrap();
    assert_eq!(value["state"], "ranked");
    let list = value["ranked_books"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(|row| row["place"] == 1 && row["tied"] == true));
    let page = send(
        &app,
        request("GET", "/me/rankings", "", &cookies, ORIGIN),
    )
    .await;
    let html = body_of(page).await;
    assert!(html.contains("Мои книги") && html.contains("Равнозначны"));
    assert!(!html.contains("Пока нет книг для сравнения"));
    let retry = send(
        &app,
        json_request(
            "POST",
            "/api/v1/me/rankings/choice",
            &json,
            &cookies,
            ORIGIN,
            &token,
        ),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::CONFLICT);
    assert_eq!(repo.snapshot(uid).await.unwrap().ranks.len(), 2);
    for path in ["/api/v1/me/rankings/start", "/api/v1/me/rankings/completed"] {
        let response = send(&app, request("POST", path, "", &cookies, ORIGIN)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_05_http_browser_form_skip_and_resume() {
    let (app, repo, uid, session) = fixture("form").await;
    let before = ready(&repo, uid, [1, 901]).await;
    let q = before.next_question().unwrap().unwrap();
    let (csrf_cookie, token) = csrf(&app, &session).await;
    let cookies = format!("{session}; {csrf_cookie}");
    let html =
        body_of(send(&app, request("GET", "/me/rankings", "", &cookies, ORIGIN)).await).await;
    assert!(html.contains("Первая лучше") && html.contains("Отложить вторую"));
    assert!(!html.contains("<script"));
    let form=format!(
        "csrf_token={token}&expected_revision={}&expected_state_version={}&comparison_id={}&outcome=skip",
        before.revision,q.state_version,q.comparison_id,
    );
    let skipped = send(
        &app,
        request("POST", "/me/rankings/choice", &form, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(skipped.status(), StatusCode::SEE_OTHER);
    let after = repo.snapshot(uid).await.unwrap();
    assert!(after.next_question().unwrap().is_none());
    let html =
        body_of(send(&app, request("GET", "/me/rankings", "", &cookies, ORIGIN)).await).await;
    assert!(html.contains("Все предложенные пары пропущены"));
    let form = format!("csrf_token={token}&expected_revision={}", after.revision);
    let resumed = send(
        &app,
        request("POST", "/me/rankings/resume", &form, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(resumed.status(), StatusCode::SEE_OTHER);
    let current = repo.snapshot(uid).await.unwrap();
    assert_ne!(
        q.comparison_id,
        current.next_question().unwrap().unwrap().comparison_id
    );
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures"]
async fn f_05_http_hidden_book_denied_without_writing_comparison() {
    let (app, repo, uid, session) = fixture("hidden").await;
    // Obtain CSRF from an empty authorized dashboard BEFORE seeding an
    // inaccessible book. Once hidden state exists, every ranking GET fails closed.
    let (csrf_cookie, token) = csrf(&app, &session).await;
    // Book 2 is a CI draft; only this test fixture may seed such stale state.
    let before = ready(&repo, uid, [1, 2]).await;
    let q = before.next_question().unwrap().unwrap();
    let cookies = format!("{session}; {csrf_cookie}");
    let response = send(
        &app,
        request("GET", "/api/v1/me/rankings", "", &cookies, ORIGIN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(body_of(response).await.is_empty());
    let json = format!(
        r#"{{"expected_revision":{},"expected_state_version":{},"comparison_id":"{}","outcome":"a_wins"}}"#,
        before.revision, q.state_version, q.comparison_id,
    );
    let denied = send(
        &app,
        json_request(
            "POST",
            "/api/v1/me/rankings/choice",
            &json,
            &cookies,
            ORIGIN,
            &token,
        ),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    assert_eq!(repo.snapshot(uid).await.unwrap(), before);
}
