//! E-02: browser-style HTTP comments with real disposable PostgreSQL.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    process::Command,
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

fn pg_blocking(url: &str, statement: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let result = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=10000 -c lock_timeout=5000",
        )
        .env("PGCONNECT_TIMEOUT", "5")
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
            statement,
        ])
        .output()
        .expect("PostgreSQL client");
    assert!(
        result.status.success(),
        "fixture SQL failed: {}",
        String::from_utf8_lossy(&result.stderr),
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

// psql may wait for a SQLx rollback queued by a rejected HTTP command.
// Keep the single-thread Tokio runtime polling while the fixture process waits.
async fn pg(url: &str, statement: &str) -> String {
    let url = url.to_owned();
    let statement = statement.to_owned();
    tokio::task::spawn_blocking(move || pg_blocking(&url, &statement))
        .await
        .expect("fixture process task")
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    body: &str,
) -> Response {
    let request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn html(reply: Response) -> String {
    String::from_utf8(
        to_bytes(reply.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL ending in /tor_ci"]
async fn e_02_http_comment_session_csrf_replay_and_hold() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated test database");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let session = generate_session_token();
    let csrf = generate_session_token();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user = accounts
        .register_with_session(
            &format!("e02-http-{nonce}@tor-ci.example"),
            "Reader",
            &hash_password("E02OnlyTestingPassword2026").unwrap(),
            &session_token_digest(&session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:e02:http:{nonce}','ru','Книга <script>','public') RETURNING id"
        ),
    )
    .await
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let comments_path = format!("/books/{book}/comments");
    let anonymous = call(&app, "GET", &comments_path, "", ORIGIN, "").await;
    assert_eq!(anonymous.status(), StatusCode::OK);
    let anonymous_html = html(anonymous).await;
    assert!(anonymous_html.contains("Войти, чтобы оставить комментарий"));
    assert!(!anonymous_html.contains("name=\"csrf_token\""));

    let detail = html(call(&app, "GET", &format!("/books/{book}"), "", ORIGIN, "").await).await;
    assert!(detail.contains(&format!("href=\"{comments_path}\"")));

    let cookies = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let page = call(&app, "GET", &comments_path, &cookies, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    let page_html = html(page).await;
    assert!(page_html.contains("Книга &lt;script&gt;"));
    let key = page_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(key.len(), 43);
    let form = format!("body=Hello%20%3Cscript%3E%20%7C%7CSECRET%20%3Cimg%20src%3Dx%20onerror%3Dalert%281%29%3E%20%40private_user%7C%7C&command_key={key}&csrf_token={csrf}");

    assert_eq!(
        call(&app, "POST", &comments_path, "", ORIGIN, &form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &cookies,
            "https://evil.invalid",
            &form,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &cookies,
            ORIGIN,
            &form.replace(&csrf, "forged"),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &format!("tor-dev-csrf={csrf}"),
            ORIGIN,
            &form,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &comments_path, &cookies, ORIGIN, &form)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &cookies,
            ORIGIN,
            &form.replace("Hello", "Other"),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let visible = html(call(&app, "GET", &comments_path, "", ORIGIN, "").await).await;
    assert!(visible.contains("Hello &lt;script&gt;"));
    assert!(!visible.contains("Hello <script>"));
    assert!(visible.contains("<details class=\"comment-spoiler\">"));
    assert!(!visible.contains("<details open"));
    assert!(visible.contains("SECRET &lt;img src=x onerror=alert(1)&gt; @пользователь"));
    assert!(!visible.contains("<img src=x"));
    assert!(!visible.contains("@private_user"));
    let count: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM discussion_comments WHERE book_id={book}"),
    )
    .await
    .parse()
    .unwrap();
    assert_eq!(count, 1);

    let parent: i64 = pg(
        &url,
        &format!("SELECT id FROM discussion_comments WHERE book_id={book}"),
    )
    .await
    .parse()
    .unwrap();
    let reply_form = format!(
        "body=Reply%20%7C%7Csecret%7C%7C&parent_id={parent}&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    let own_page = html(call(&app, "GET", &comments_path, &cookies, ORIGIN, "").await).await;
    assert!(own_page.contains(&format!("Ваш ответ на комментарий №{parent}")));
    assert!(!visible.contains("Отправить ответ"));
    for (origin, body, expected) in [
        (
            "https://evil.invalid",
            reply_form.clone(),
            StatusCode::FORBIDDEN,
        ),
        (
            ORIGIN,
            reply_form.replace(&csrf, "forged"),
            StatusCode::FORBIDDEN,
        ),
        (
            ORIGIN,
            reply_form.replace(&format!("parent_id={parent}"), "parent_id=0"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ORIGIN,
            reply_form.replace(
                &format!("parent_id={parent}"),
                "parent_id=9223372036854775807",
            ),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            call(&app, "POST", &comments_path, &cookies, origin, &body)
                .await
                .status(),
            expected
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &format!("tor-dev-csrf={csrf}"),
            ORIGIN,
            &reply_form
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &comments_path, &cookies, ORIGIN, &reply_form)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &cookies,
            ORIGIN,
            &reply_form.replace("Reply", "Changed")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let replies_page = html(call(&app, "GET", &comments_path, "", ORIGIN, "").await).await;
    assert!(replies_page.contains(&format!("Ответ на комментарий №{parent}")));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM discussion_comments WHERE parent_id={parent}")
        )
        .await,
        "1"
    );
    let writer_session = generate_session_token();
    let writer_csrf = generate_session_token();
    let writer_id = PgAccounts::from_pool(pool.clone())
        .register_with_session(
            &format!("e03-recipient-{nonce}@tor-ci.example"),
            "Other reader",
            &hash_password("E03OnlyTestingPassword2026").unwrap(),
            &session_token_digest(&writer_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let writer_cookie = format!("tor-dev-session={writer_session}; tor-dev-csrf={writer_csrf}");
    let incoming_form = format!(
        "body=NEVER_COPY_THIS_SPOILER&parent_id={parent}&command_key={}&csrf_token={writer_csrf}",
        generate_session_token()
    );
    for _ in 0..2 {
        assert_eq!(
            call(
                &app,
                "POST",
                &comments_path,
                &writer_cookie,
                ORIGIN,
                &incoming_form
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
    }
    let incoming_id: i64 = pg(&url,&format!("SELECT id FROM discussion_comments WHERE author_user_id={writer_id} AND book_id={book}")).await.parse().unwrap();
    let link = format!(
        "/books/{book}/comments?after={}#comment-{incoming_id}",
        incoming_id + 1
    );
    let account = call(&app, "GET", "/me", &cookies, ORIGIN, "").await;
    assert_eq!(account.status(), StatusCode::OK);
    assert!(account
        .headers()
        .get(header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("no-store"));
    let account_html = html(account).await;
    assert_eq!(account_html.matches(&link).count(), 1);
    assert!(account_html.contains("Книга &lt;script&gt;"));
    assert!(!account_html.contains("NEVER_COPY_THIS_SPOILER"));
    assert!(account_html.contains("Не прочитано"));
    // The standalone Inbox is private and never includes the comment body.
    assert_eq!(
        call(&app, "GET", "/api/v1/me/inbox", "", ORIGIN, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let inbox_path = "/me/inbox?filter=unread";
    let inbox = call(&app, "GET", inbox_path, &cookies, ORIGIN, "").await;
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox_html = html(inbox).await;
    assert!(inbox_html.contains("Входящие обсуждения"));
    assert!(inbox_html.contains(&link));
    assert!(!inbox_html.contains("NEVER_COPY_THIS_SPOILER"));
    let json = html(call(&app, "GET", "/api/v1/me/inbox", &cookies, ORIGIN, "").await).await;
    let item: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(item["items"][0]["id"], incoming_id.to_string());
    assert_eq!(item["items"][0]["kind"], "reply");
    assert_eq!(item["items"][0]["is_read"], false);
    assert!(!json.contains("NEVER_COPY_THIS_SPOILER"));
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/me/inbox?filter=unknown",
            &cookies,
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/me/inbox?limit=51",
            &cookies,
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let read_path = format!("/me/replies/{incoming_id}/read-state");
    let read_form = format!("csrf_token={csrf}&state=read");
    assert_eq!(
        call(
            &app,
            "POST",
            &read_path,
            &cookies,
            "https://evil.invalid",
            &read_form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &read_path,
            &writer_cookie,
            ORIGIN,
            &format!("csrf_token={writer_csrf}&state=read")
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &read_path, &cookies, ORIGIN, &read_form)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert!(html(call(&app, "GET", "/me", &cookies, ORIGIN, "").await)
        .await
        .contains("Отметить непрочитанным"));
    let read_inbox = html(
        call(
            &app,
            "GET",
            "/api/v1/me/inbox?filter=read",
            &cookies,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(read_inbox.contains(&incoming_id.to_string()));
    assert!(!html(
        call(
            &app,
            "GET",
            "/api/v1/me/inbox?filter=unread",
            &cookies,
            ORIGIN,
            ""
        )
        .await,
    )
    .await
    .contains(&format!("\"id\":\"{incoming_id}\"")));
    for _ in 0..2 {
        assert_eq!(
            call(
                &app,
                "POST",
                &read_path,
                &cookies,
                ORIGIN,
                &format!("csrf_token={csrf}&state=unread")
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert!(html(call(&app, "GET", "/me", &cookies, ORIGIN, "").await)
        .await
        .contains("Отметить прочитанным"));
    assert_eq!(
        call(
            &app,
            "POST",
            &read_path,
            &cookies,
            ORIGIN,
            "csrf_token=invalid&state=read"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    assert!(
        !html(call(&app, "GET", "/me", &writer_cookie, ORIGIN, "").await)
            .await
            .contains(&link)
    );
    assert_eq!(
        call(&app, "GET", "/me", "", ORIGIN, "").await.status(),
        StatusCode::SEE_OTHER
    );
    pg(
        &url,
        &format!("UPDATE discussion_comments SET visibility='hidden' WHERE id={parent}"),
    )
    .await;
    assert!(!html(call(&app, "GET", "/me", &cookies, ORIGIN, "").await)
        .await
        .contains(&link));

    assert_eq!(
        call(&app, "POST", &comments_path, &cookies, ORIGIN, &reply_form)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    )
    .await;
    assert_eq!(
        call(&app, "GET", &comments_path, "", ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let held_form = format!(
        "body=Held&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    assert_eq!(
        call(&app, "POST", &comments_path, &cookies, ORIGIN, &held_form)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    )
    .await;
    // E-03: a selected participant without a public handle is addressed by ID,
    // but must be validated server-side before creating an Inbox reference.
    let discover_url = format!("/api/v1/social/mentions/discover?book_id={book}&q=Ot");
    assert_eq!(
        call(&app, "GET", &discover_url, "", ORIGIN, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let candidates = html(call(&app, "GET", &discover_url, &cookies, ORIGIN, "").await).await;
    assert!(candidates.contains(&format!("\"user_id\":{writer_id}")));
    let mention_key = generate_session_token();
    let mention_form = format!(
        "body=Look%20%40%5Buser%3A{writer_id}%5D&command_key={mention_key}&csrf_token={csrf}"
    );
    for _ in 0..2 {
        assert_eq!(
            call(
                &app,
                "POST",
                &comments_path,
                &cookies,
                ORIGIN,
                &mention_form
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
    }
    let mention_id: i64 = pg(
        &url,
        &format!(
        "SELECT id FROM discussion_comments WHERE book_id={book} AND command_key='{mention_key}'"
    ),
    )
    .await
    .parse()
    .unwrap();
    assert_eq!(pg(&url, &format!(
        "SELECT count(*) FROM discussion_comment_mentions WHERE comment_id={mention_id} AND recipient_user_id={writer_id}"
    )).await, "1");
    let notification =
        html(call(&app, "GET", "/api/v1/me/inbox", &writer_cookie, ORIGIN, "").await).await;
    assert!(notification.contains("\"kind\":\"mention\""));
    assert!(notification.contains(&format!("\"id\":\"{mention_id}\"")));
    assert!(!notification.contains("Look"));
    pg(
        &url,
        &format!("DELETE FROM discussion_comments WHERE id={mention_id}"),
    )
    .await;
    pg(
        &url,
        &format!("DELETE FROM discussion_comments WHERE parent_id={parent}"),
    )
    .await;
    pg(&url, &format!("DELETE FROM users WHERE id={writer_id}")).await;
    pg(&url, &format!("DELETE FROM users WHERE id={user}")).await;
    pg(&url, &format!("DELETE FROM books WHERE id={book}")).await;
}
