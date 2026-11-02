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
    assert!(list
        .iter()
        .all(|row| row["place"] == 1 && row["tied"] == true));
    let page = send(&app, request("GET", "/me/rankings", "", &cookies, ORIGIN)).await;
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

fn completion_field(page: &str, field: &str) -> String {
    let section = page
        .split("action=\"/read/101/complete\"")
        .nth(1)
        .expect("one explicit completion form")
        .split("</form>")
        .next()
        .unwrap();
    section
        .split(&format!("name=\"{field}\" value=\""))
        .nth(1)
        .expect("server-supplied completion field")
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL fixtures and psql client"]
async fn f_05_http_reader_finish_form_to_personal_ranking() {
    let (app, repo, uid, session) = fixture("complete").await;
    let initial = send(&app, request("GET", "/read/101", "", &session, ORIGIN)).await;
    assert_eq!(initial.status(), StatusCode::OK);
    let csrf_cookie = cookie(&initial, "tor-dev-csrf=");
    let page = body_of(initial).await;
    assert_eq!(page.matches("action=\"/read/101/complete\"").count(), 1);
    assert!(page.contains("Глава прочитана"));
    assert_eq!(completion_field(&page, "expected_revision"), "0");
    let csrf = completion_field(&page, "csrf_token");
    let version = completion_field(&page, "version_id");
    let cookies = format!("{session}; {csrf_cookie}");
    assert_eq!(repo.snapshot(uid).await.unwrap().ranks.len(), 0);

    // Saving a position with the legacy button never adds a book to the ranking.
    let saved = format!("csrf_token={csrf}&version_id={version}&block_index=1&char_offset=0");
    assert_eq!(
        send(
            &app,
            request("POST", "/read/101/place", &saved, &cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(repo.snapshot(uid).await.unwrap().ranks.is_empty());

    // A stale tab cannot assert chapter completion using the old revision.
    let reloaded = send(&app, request("GET", "/read/101", "", &session, ORIGIN)).await;
    assert_eq!(reloaded.status(), StatusCode::OK);
    let fresh_html = body_of(reloaded).await;
    assert_eq!(completion_field(&fresh_html, "expected_revision"), "1");
    let last_block = completion_field(&fresh_html, "block_index");
    let last_offset = completion_field(&fresh_html, "char_offset");
    assert_ne!(last_offset, "0");
    let fresh = format!(
        "csrf_token={csrf}&version_id={version}&block_index={last_block}&char_offset={last_offset}&expected_revision=1"
    );
    let stale = fresh.replace("expected_revision=1", "expected_revision=0");
    assert_eq!(
        send(
            &app,
            request("POST", "/read/101/complete", &stale, &cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/read/101/complete",
                &fresh,
                &cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let wrong_offset = fresh.replace(&format!("char_offset={last_offset}"), "char_offset=0");
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/read/101/complete",
                &wrong_offset,
                &cookies,
                ORIGIN
            )
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(repo.snapshot(uid).await.unwrap().ranks.is_empty());

    let completed = send(
        &app,
        request("POST", "/read/101/complete", &fresh, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(completed.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        completed.headers()[header::LOCATION],
        "/read/101#continuation"
    );
    assert_eq!(
        send(
            &app,
            request("POST", "/read/101/complete", &fresh, &cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "repeat form must not create a second completion event"
    );

    // Integration runner has PostgreSQL client; fetch the C-owned event ID,
    // then execute exactly the same F consumer as the background worker.
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"), "never run against production");
    let sql = format!(
        "SELECT source_event_id FROM reader_chapter_completions WHERE user_id={uid} AND chapter_id=101"
    );
    let output = std::process::Command::new("psql")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1"])
        .args(["-d", &url, "-c", &sql])
        .output()
        .expect("psql available in integration CI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let event = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    assert_eq!(event.len(), 36);
    // The HTML POST invokes F's trusted consumer before redirecting. The
    // persistent outbox receipt also prevents the background worker replay.
    assert!(repo.snapshot(uid).await.unwrap().ranks.len() == 1);
    assert!(!repo
        .pending_completion_event_ids()
        .await
        .unwrap()
        .contains(&event));
    assert!(!repo.accept_completed_chapter(&event).await.unwrap());

    let reopened = send(&app, request("GET", "/read/101", "", &session, ORIGIN)).await;
    let reopened_html = body_of(reopened).await;
    assert!(reopened_html.contains("Глава уже отмечена прочитанной"));
    assert!(!reopened_html.contains("action=\"/read/101/complete\""));

    // F-12/S046: this very same verified C event also awards the first
    // private reading badge; pageview and saved place alone did not.
    let private = send(&app, request("GET", "/me", "", &session, ORIGIN)).await;
    assert_eq!(private.status(), StatusCode::OK);
    assert_eq!(private.headers()[header::CACHE_CONTROL], "no-store");
    let dashboard = body_of(private).await;
    assert!(dashboard.contains("Первая глава"));
    assert!(dashboard.contains("Получено: 1/1 глав"));
    assert!(dashboard.contains("Дата получения:"));
    assert_eq!(
        send(&app, request("GET", "/me", "", "", ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER,
        "guest must never see another reader's achievements"
    );

    let result = send(&app, request("GET", "/me/rankings", "", &session, ORIGIN)).await;
    assert_eq!(result.status(), StatusCode::OK);
    let html = body_of(result).await;
    assert!(html.contains("Мои книги"));
    assert!(html.contains("Место 1"));
    assert_eq!(repo.snapshot(uid).await.unwrap().ranks.len(), 1);
}
