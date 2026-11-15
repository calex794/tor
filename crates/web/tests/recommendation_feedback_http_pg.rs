//! Real A/B/C/F HTTP acceptance on disposable PG. No live user or deployment.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use serde_json::{json, Value};
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
#[path = "recommendation_feedback_http_pg/command_checks.rs"]
mod command_checks;
#[path = "support/collection_retirement.rs"]
mod retirement;
use retirement::{assert_current_schema, optional_snapshot};
use tower::ServiceExt;
const ORIGIN: &str = "http://127.0.0.1:3098";
fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let out = Command::new("psql")
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
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    auth: (&str, &str),
    body: &str,
    json_body: bool,
) -> Response {
    let (origin, csrf) = auth;
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header("x-csrf-token", csrf)
        .header(
            header::CONTENT_TYPE,
            if json_body {
                "application/json"
            } else {
                "application/x-www-form-urlencoded"
            },
        )
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}
async fn feedback_json(
    app: &Router,
    cookie: &str,
    csrf: &str,
    key: Option<&str>,
    body: &str,
) -> Response {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/me/recommendation-feedback")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header("x-csrf-token", csrf)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}
async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn data(response: Response) -> Value {
    serde_json::from_str(&text(response).await).unwrap()
}

fn private(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let robots = response.headers()["x-robots-tag"].to_str().unwrap();
    assert!(robots.contains("noindex") && robots.contains("nofollow"));
}
async fn assert_retired(response: Response) {
    assert_eq!(response.status(), StatusCode::GONE);
    private(&response);
    assert_eq!(data(response).await["error"]["code"], "COLLECTION_RETIRED");
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn rec13_retirement_preserves_business_settings_reading_preferences_and_guards() {
    std::env::set_var("TOR_RECOMMENDATIONS_ENABLED", "1");
    std::env::set_var("TOR_REC_ATTRIBUTION_ENABLED", "1");
    std::env::set_var("TOR_ORGANIC_ATTRIBUTION_ENABLED", "1");
    for name in [
        "TOR_FLOW_VISITS_ENABLED",
        "TOR_READING_RETURNS_ENABLED",
        "TOR_BOOK_READING_RETURNS_ENABLED",
    ] {
        std::env::set_var(name, "1");
    }
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let session = generate_session_token();
    let other_session = generate_session_token();
    let csrf = generate_session_token();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("Rec13VerifiedBrowserPassword2026").unwrap();
    let user = accounts
        .register_with_session(
            &format!("rec13-http-{nonce}@tor-ci.example"),
            "Reader",
            &password,
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let other = accounts
        .register_with_session(
            &format!("rec13-other-{nonce}@tor-ci.example"),
            "Other",
            &password,
            &session_token_digest(&other_session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book:i64=pg(&url,&format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id) VALUES('ci:rec13:http:{nonce}','ru','Book <script>unsafe</script>','public','basic',{user}) RETURNING id")).parse().unwrap();
    let expected_book_id = book.to_string();
    let next:i64=pg(&url,&format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES('ci:rec13:next:{nonce}','ru','Next book','public','basic') RETURNING id")).parse().unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO book_recommendation_audiences(book_id,audience) VALUES({book},'general')"
        ),
    );
    let chapter:i64=pg(&url,&format!("INSERT INTO chapters(book_id,chapter_no,title,visibility) VALUES({book},1,'First','public') RETURNING id")).parse().unwrap();
    let version:i64=pg(&url,&format!("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES({chapter},1,'Readable text') RETURNING id")).parse().unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let other_cookie = format!("tor-dev-session={other_session}; tor-dev-csrf={csrf}");
    // Published basic books are eligible without a recommendation-specific
    // manual review, including existing books without an audience row.
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_public_books WHERE id IN ({book},{next})")
        ),
        "2"
    );
    let owner_form = format!("audience=general&related=&expected_revision=1&csrf_token={csrf}");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/me/books/{book}/recommendations"),
            &cookie,
            (ORIGIN, &csrf),
            &owner_form,
            false
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    pg(
        &url,
        &format!("UPDATE books SET title='Book <script>unsafe</script> revised' WHERE id={book}"),
    );
    pg(&url, &format!("UPDATE chapters SET title='Edited chapter',edit_revision=edit_revision+1 WHERE id={chapter}"));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_public_books WHERE id IN ({book},{next})")
        ),
        "2",
        "Editing public content must not force an extra moderation step"
    );
    let adult_form = format!("audience=adult&related=&expected_revision=2&csrf_token={csrf}");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/me/books/{book}/recommendations"),
            &cookie,
            (ORIGIN, &csrf),
            &adult_form,
            false
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_public_books WHERE id={book}")
        ),
        "0"
    );
    let general_form = format!("audience=general&related=&expected_revision=3&csrf_token={csrf}");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/me/books/{book}/recommendations"),
            &cookie,
            (ORIGIN, &csrf),
            &general_form,
            false
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    pg(
        &url,
        &format!("UPDATE books SET access_level='adult' WHERE id={book}"),
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_public_books WHERE id={book}")
        ),
        "0"
    );
    pg(
        &url,
        &format!("UPDATE books SET access_level='basic' WHERE id={book}"),
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_public_books WHERE id IN ({book},{next})")
        ),
        "2"
    );
    // The administrator role is checked independently for control endpoints.
    pg(
        &url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({other},'administrator')"),
    );
    let admin_form = format!("audience=general&related=&expected_revision=4&csrf_token={csrf}");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/me/books/{book}/recommendations"),
            &other_cookie,
            (ORIGIN, &csrf),
            &admin_form,
            false
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_public_books WHERE id IN ({book},{next})")
        ),
        "2"
    );
    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={other} AND role='administrator'"));
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/admin/recommendations/quality",
            &cookie,
            (ORIGIN, &csrf),
            "",
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/me/recommendations/manage",
            "",
            (ORIGIN, ""),
            "",
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let no_consent = data(
        call(
            &app,
            "GET",
            "/api/v1/recommendations",
            &cookie,
            (ORIGIN, &csrf),
            "",
            false,
        )
        .await,
    )
    .await;
    assert!(no_consent["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i.get("ticket").is_none()));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_subjects WHERE user_id IN({user},{other})")
        ),
        "0"
    );
    assert_current_schema(&url);
    let before = optional_snapshot(&url, user);
    let other_before = optional_snapshot(&url, other);
    command_checks::cached_clients(&app, &cookie, &other_cookie, &csrf, other).await;
    let opened = call(
        &app,
        "GET",
        &format!("/books/{book}"),
        &cookie,
        (ORIGIN, &csrf),
        "",
        false,
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let markup = text(opened).await;
    assert!(!markup.contains("<script>unsafe</script>"));
    assert!(markup.contains("data-rec-panel"));
    assert!(!markup.contains("data-rec-ticket"));
    assert!(!markup.contains("data-organic-ticket"));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM reader_chapter_starts WHERE user_id={user}")
        ),
        "0"
    );
    let start = json!({"chapter_id":chapter.to_string(),"version_id":version.to_string(),
        "block_id":1,"char_offset":3,"expected_revision":0})
    .to_string();
    let started = call(
        &app,
        "PUT",
        "/api/v1/me/progress/free",
        &cookie,
        (ORIGIN, &csrf),
        &start,
        true,
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    let receipt = data(started).await;
    let finish = json!({"chapter_id":chapter.to_string(),"version_id":version.to_string(),
        "block_id":1,"char_offset":13,"expected_revision":receipt["revision"].as_i64().unwrap()})
    .to_string();
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/v1/me/progress/free",
            &cookie,
            (ORIGIN, &csrf),
            &finish,
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT
        (SELECT count(*) FROM reader_chapter_starts WHERE user_id={user})||'|'||
        (SELECT count(*) FROM reader_chapter_completions WHERE user_id={user})"
            )
        ),
        "1|1"
    );
    assert_eq!(optional_snapshot(&url, user), before);
    assert_eq!(optional_snapshot(&url, other), other_before);

    let key = generate_session_token();
    let forbidden=json!({"book_id":book.to_string(),"choice":"not_interested","expected_revision":0,"user_id":other}).to_string();
    assert_eq!(
        feedback_json(&app, &cookie, &csrf, Some(&key), &forbidden)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let hide = json!({"book_id":book.to_string(),"choice":"not_interested","expected_revision":0})
        .to_string();
    let old_body=json!({"book_id":book.to_string(),"choice":"not_interested","expected_revision":0,"command_key":key}).to_string();
    assert_eq!(
        feedback_json(&app, &cookie, &csrf, Some(&key), &old_body)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "The deprecated JSON body key is a forbidden extra field"
    );
    let missing = feedback_json(&app, &cookie, &csrf, None, &hide).await;
    assert_eq!(missing.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        data(missing).await["error"]["code"],
        "IDEMPOTENCY_KEY_REQUIRED"
    );
    let noncanonical_key = "x".repeat(43);
    for invalid in ["bad", noncanonical_key.as_str()] {
        let rejected = feedback_json(&app, &cookie, &csrf, Some(invalid), &hide).await;
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            data(rejected).await["error"]["code"],
            "INVALID_IDEMPOTENCY_KEY"
        );
    }
    for _ in 0..2 {
        assert_eq!(
            feedback_json(&app, &cookie, &csrf, Some(&key), &hide)
                .await
                .status(),
            StatusCode::OK
        );
    }
    let changed =
        json!({"book_id":book.to_string(),"choice":"clear","expected_revision":0}).to_string();
    let collision = feedback_json(&app, &cookie, &csrf, Some(&key), &changed).await;
    assert_eq!(
        collision.status(),
        StatusCode::CONFLICT,
        "One actor and key cannot change its semantic payload"
    );
    assert_eq!(
        data(collision).await["error"]["code"],
        "IDEMPOTENCY_CONFLICT"
    );
    let after = data(
        call(
            &app,
            "GET",
            "/api/v1/recommendations",
            &cookie,
            (ORIGIN, &csrf),
            "",
            false,
        )
        .await,
    )
    .await;
    assert!(after["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["book_id"].as_str() != Some(expected_book_id.as_str())));
    let their = data(
        call(
            &app,
            "GET",
            "/api/v1/recommendations",
            &other_cookie,
            (ORIGIN, &csrf),
            "",
            false,
        )
        .await,
    )
    .await;
    assert!(their["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["book_id"].as_str() == Some(expected_book_id.as_str())));
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/me/books/{book}/recommendations"),
            &other_cookie,
            (ORIGIN, &csrf),
            "",
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    command_checks::administrator(&app, &cookie, &csrf, &url, user, book).await;
    let revoke = format!(
        "expected_revision=7&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    assert_retired(
        call(
            &app,
            "POST",
            "/me/recommendations/consent",
            &cookie,
            (ORIGIN, &csrf),
            &revoke,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(optional_snapshot(&url, user), before);
    assert_eq!(optional_snapshot(&url, other), other_before);
    assert_eq!(pg(&url, &format!("SELECT count(*) FROM reader_chapter_completions WHERE user_id={user} AND book_id={book}")), "1");
    pg(
        &url,
        &format!(
            "DELETE FROM rec_subjects WHERE user_id IN({user},{other});
        DELETE FROM account_roles WHERE user_id IN({user},{other});
        UPDATE books SET admin_hold=true WHERE id IN({book},{next})"
        ),
    );
}
