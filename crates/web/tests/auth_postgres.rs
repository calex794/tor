//! A-02/A-03: protected forms and revocable sessions on disposable PostgreSQL.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::net::SocketAddr;
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn request(method: &str, uri: &str, body: &str, cookies: &str, origin: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .body(Body::from(body.to_owned()))
        .expect("HTTP");
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40001))));
    req
}
async fn send(app: &Router, req: Request<Body>) -> Response {
    app.clone().oneshot(req).await.expect("router response")
}
async fn body_of(reply: Response) -> String {
    let body = to_bytes(reply.into_body(), 128 * 1024).await.expect("body");
    String::from_utf8(body.to_vec()).expect("HTML UTF-8")
}
fn csrf_token(html: &str) -> &str {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .expect("csrf form field")
        .split('"')
        .next()
        .expect("csrf value")
}
fn cookie(reply: &Response, prefix: &str) -> String {
    reply
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.starts_with(prefix)
                .then(|| s.split(';').next().unwrap_or_default().to_owned())
        })
        .expect("Set-Cookie")
}
#[tokio::test]
#[ignore = "Requires TEST_DATABASE_URL on isolated PostgreSQL tor_ci"]
async fn registration_login_logout_origin_csrf_and_private_profile() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database URL");
    let catalog = PgCatalog::connect(&url).await.expect("role and schema");
    let pool = catalog.pool();
    let reader = PgReader::new(pool.clone());
    reader.probe().await.expect("reading schema");
    let accounts = PgAccounts::from_pool(pool.clone());
    accounts.probe().await.expect("accounts schema");
    assert!(
        url.ends_with("/tor_ci"),
        "Never run auth integration against production"
    );
    let lookup = accounts.clone();

    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let page = send(&app, request("GET", "/register", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let page_body = body_of(page).await;
    let csrf = csrf_token(&page_body);
    assert!(page_body.contains("Создать учётную запись"));

    let form = "email=tor_ci_account%40example.com&display_name=CIReader&password=LongSecurePassword2026&csrf_token=".to_owned() + csrf;
    assert_eq!(
        send(&app, request("POST", "/register", &form, "", ORIGIN))
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "Registration requires browser CSRF token cookie"
    );
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/register",
                &form,
                &csrf_cookie,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "Origin cannot be overridden by the CSRF token"
    );
    let created = send(
        &app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    assert_eq!(created.headers()[header::LOCATION], "/me");
    let session = cookie(&created, "tor-dev-session=");
    assert!(session.contains("tor-dev-session="));
    assert!(created
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().unwrap().contains("HttpOnly")));

    let hash = lookup
        .find_user("tor_ci_account@example.com")
        .await
        .expect("account lookup")
        .expect("persisted account")
        .password_hash;
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains("LongSecurePassword2026"));

    let private = send(&app, request("GET", "/me", "", &session, ORIGIN)).await;
    assert_eq!(private.status(), StatusCode::OK);
    assert_eq!(private.headers()[header::CACHE_CONTROL], "no-store");
    let new_csrf_cookie = cookie(&private, "tor-dev-csrf=");
    let private_html = body_of(private).await;
    assert!(private_html.contains("href=\"/me/library\""));

    // B-02: a registered reader can become an author, but drafts stay private.
    let author_cookies = format!("{session}; {new_csrf_cookie}");
    let anonymous_form = send(&app, request("GET", "/books/new", "", "", ORIGIN)).await;
    assert_eq!(anonymous_form.status(), StatusCode::SEE_OTHER);
    let author_form = send(
        &app,
        request("GET", "/books/new", "", &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(author_form.status(), StatusCode::OK);
    assert_eq!(author_form.headers()[header::CACHE_CONTROL], "no-store");
    let form_html = body_of(author_form).await;
    let book_csrf = csrf_token(&form_html);
    let command_key = form_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .expect("author idempotency key")
        .split('"')
        .next()
        .expect("key content");
    assert_eq!(command_key.len(), 43);
    let author_payload = format!(
        "title=AuthorDraftCI&source_identity=tor-ci%3Aauthor%3Aweb&target_language=ru&command_key={command_key}&csrf_token={book_csrf}"
    );
    assert_eq!(
        send(
            &app,
            request("POST", "/books/new", &author_payload, &session, ORIGIN)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/books/new",
                &author_payload,
                &author_cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let submitted = send(
        &app,
        request("POST", "/books/new", &author_payload, &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(submitted.status(), StatusCode::SEE_OTHER);
    let author_url = submitted.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(author_url.starts_with("/author/books/"));
    let author_page = send(
        &app,
        request("GET", &author_url, "", &session, ORIGIN),
    )
    .await;
    assert_eq!(author_page.status(), StatusCode::OK);
    assert_eq!(author_page.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_of(author_page).await.contains("AuthorDraftCI"));
    assert_eq!(
        send(&app, request("GET", &author_url, "", "", ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let new_book_id = author_url.rsplit('/').next().unwrap();
    assert_eq!(
        send(
            &app,
            request("GET", &format!("/books/{new_book_id}"), "", "", ORIGIN)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let own_list = send(&app, request("GET", "/me/books", "", &session, ORIGIN)).await;
    assert_eq!(own_list.status(), StatusCode::OK);
    assert!(body_of(own_list).await.contains("AuthorDraftCI"));
    let replay = send(
        &app,
        request("POST", "/books/new", &author_payload, &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::SEE_OTHER);
    assert_eq!(replay.headers()[header::LOCATION], author_url);
    let changed = author_payload.replace("AuthorDraftCI", "ChangedDraftCI");
    assert_eq!(
        send(
            &app,
            request("POST", "/books/new", &changed, &author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let anonymous_library = send(&app, request("GET", "/me/library", "", "", ORIGIN)).await;
    assert_eq!(anonymous_library.status(), StatusCode::SEE_OTHER);
    assert_eq!(anonymous_library.headers()[header::LOCATION], "/login");
    let private_library = send(&app, request("GET", "/me/library", "", &session, ORIGIN)).await;
    assert_eq!(private_library.status(), StatusCode::OK);
    assert_eq!(private_library.headers()[header::CACHE_CONTROL], "no-store");
    let library_html = body_of(private_library).await;
    assert!(library_html.contains("Вы пока не сохраняли место чтения"));
    assert!(!library_html.contains("PRIVATE-PAID-MARKER"));
    // Authentication and ACL are enforced on every place write, independent of
    // links shown to the reader. Never trust user_id from a request.
    let anonymous_read = send(&app, request("GET", "/read/101", "", "", ORIGIN)).await;
    assert_eq!(anonymous_read.status(), StatusCode::SEE_OTHER);
    let authenticated_read = send(
        &app,
        request(
            "GET",
            "/read/101",
            "",
            &format!("{session}; {new_csrf_cookie}"),
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(authenticated_read.status(), StatusCode::OK);
    let read_html = body_of(authenticated_read).await;
    assert!(read_html.contains("Сохранить место здесь"));
    assert!(read_html.contains("name=\"version_id\" value=\"201\""));
    let csrf = csrf_token(&read_html);
    let place_form = format!("csrf_token={csrf}&version_id=201&block_index=1&char_offset=0");
    assert_eq!(
        send(
            &app,
            request("POST", "/read/101/place", &place_form, &session, ORIGIN)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "missing CSRF cookie"
    );
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/read/101/place",
                &place_form,
                &format!("{session}; {new_csrf_cookie}"),
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "cross-origin must be rejected"
    );
    let saved = send(
        &app,
        request(
            "POST",
            "/read/101/place",
            &place_form,
            &format!("{session}; {new_csrf_cookie}"),
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    assert_eq!(saved.headers()[header::LOCATION], "/read/101#p-1");
    let library_after = send(&app, request("GET", "/me/library", "", &session, ORIGIN)).await;
    let after = body_of(library_after).await;
    assert!(after.contains("href=\"/read/101#p-1\""));
    assert!(!after.contains("PRIVATE-PAID-MARKER"));
    let bad_version = format!("csrf_token={csrf}&version_id=999&block_index=1&char_offset=0");
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/read/101/place",
                &bad_version,
                &format!("{session}; {new_csrf_cookie}"),
                ORIGIN
            )
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let bad_block = format!("csrf_token={csrf}&version_id=201&block_index=201&char_offset=0");
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/read/101/place",
                &bad_block,
                &format!("{session}; {new_csrf_cookie}"),
                ORIGIN
            )
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    for hidden in [102, 103, 104, 105, 106, 107] {
        assert_eq!(
            send(
                &app,
                request("GET", &format!("/read/{hidden}"), "", &session, ORIGIN)
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }

    assert!(private_html.contains("CIReader"));
    assert!(!private_html.contains("LongSecurePassword2026"));
    assert!(private_html.contains("href=\"/me\""));

    assert_eq!(
        send(
            &app,
            request("POST", "/register", &form, &csrf_cookie, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "Email uniqueness enforced by PostgreSQL"
    );
    let login_page = send(&app, request("GET", "/login", "", "", ORIGIN)).await;
    let login_csrf_cookie = cookie(&login_page, "tor-dev-csrf=");
    let login_html = body_of(login_page).await;
    let login_csrf = csrf_token(&login_html);
    let bad_login =
        format!("email=tor_ci_account%40example.com&password=badpass&csrf_token={login_csrf}");
    let bad = send(
        &app,
        request("POST", "/login", &bad_login, &login_csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    let good_login = format!("email=tor_ci_account%40example.com&password=LongSecurePassword2026&csrf_token={login_csrf}");
    let logged = send(
        &app,
        request("POST", "/login", &good_login, &login_csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(logged.status(), StatusCode::SEE_OTHER);

    let logout_csrf = csrf_token(&private_html);
    let combined_cookies = format!("{session}; {new_csrf_cookie}");
    let logout = format!("csrf_token={logout_csrf}");
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/logout",
                &logout,
                &combined_cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let left = send(
        &app,
        request("POST", "/logout", &logout, &combined_cookies, ORIGIN),
    )
    .await;
    assert_eq!(left.status(), StatusCode::SEE_OTHER);
    assert!(left
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().unwrap().contains("Max-Age=0")));
    let revoked = send(&app, request("GET", "/me", "", &session, ORIGIN)).await;
    assert_eq!(revoked.status(), StatusCode::SEE_OTHER);
    assert_eq!(revoked.headers()[header::LOCATION], "/login");
}
