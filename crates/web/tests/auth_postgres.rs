//! A-02/A-03: protected forms and revocable sessions on disposable PostgreSQL.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{net::SocketAddr, sync::Arc};
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

#[path = "auth_postgres/b04_cases.rs"]
mod b04_cases;
#[path = "auth_postgres/b06_cases.rs"]
mod b06_cases;
#[path = "auth_postgres/c06_cases.rs"]
mod c06_cases;
#[path = "auth_postgres/c11_cases.rs"]
mod c11_cases;
#[path = "auth_postgres/f05_ranking_cases.rs"]
mod f05_ranking_cases;
#[path = "auth_postgres/h_account_cases.rs"]
mod h_account_cases;
#[path = "auth_postgres/h_cover_cases.rs"]
mod h_cover_cases;
#[path = "auth_postgres/http_helpers.rs"]
mod http_helpers;
use http_helpers::json_request;

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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Requires TEST_DATABASE_URL on isolated PostgreSQL tor_ci"]
async fn registration_login_logout_origin_csrf_and_private_profile() {
    tokio::time::timeout(
        std::time::Duration::from_secs(180),
        registration_login_logout_origin_csrf_and_private_profile_impl(),
    )
    .await
    .expect("auth PostgreSQL integration exceeded 180 seconds; review last checkpoint");
}

async fn registration_login_logout_origin_csrf_and_private_profile_impl() {
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

    let cover_store = h_cover_cases::MemoryCoverStore::default();
    let app = tor_web::router_with_postgres_reading_auth_cover_uploads(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
        Arc::new(cover_store.clone()),
    );
    let page = send(&app, request("GET", "/register", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let page_body = body_of(page).await;
    let csrf = csrf_token(&page_body);
    assert!(page_body.contains("Создать учётную запись"));
    assert!(page_body.contains("action=\"/register\" autocomplete=\"on\" novalidate"));
    assert!(page_body.contains("register-password-hint"));
    h_account_cases::invalid_registration(&app, csrf, &csrf_cookie).await;

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
    assert!(private_html.contains("href=\"/me/books\""));
    assert!(!private_html.contains("Приватность"));
    assert!(!private_html.contains("Анализ ваших переходов"));
    assert!(!private_html.contains("/me/behavior"));
    // B-01/B-02: the first genuine author journey persists a private book.
    let anon_new = send(&app, request("GET", "/me/books/new", "", "", ORIGIN)).await;
    assert_eq!(anon_new.status(), StatusCode::SEE_OTHER);
    assert_eq!(anon_new.headers()[header::LOCATION], "/login");
    let new_page = send(&app, request("GET", "/me/books/new", "", &session, ORIGIN)).await;
    assert_eq!(new_page.status(), StatusCode::OK);
    let author_csrf_cookie = cookie(&new_page, "tor-dev-csrf=");
    let new_html = body_of(new_page).await;
    let author_csrf = csrf_token(&new_html);
    let command_key = new_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .expect("idempotency key")
        .split('"')
        .next()
        .expect("valid key");
    assert_eq!(command_key.len(), 43);
    assert!(new_html.contains("name=\"author_name\""));
    assert!(new_html.contains("name=\"genres\""));
    let create_form = format!(
        "title=%D0%9C%D0%BE%D1%8F%20%3Cscript%3E%20%D0%BA%D0%BD%D0%B8%D0%B3%D0%B0&target_language=ru&author_name=Alice+Voyager&genres=fantasy%2Cadventure&command_key={command_key}&csrf_token={author_csrf}"
    );
    let author_cookies = format!("{session}; {author_csrf_cookie}");
    assert_eq!(
        send(
            &app,
            request("POST", "/me/books", &create_form, &session, ORIGIN)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "Author POST requires a CSRF cookie"
    );
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/me/books",
                &create_form,
                &author_cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "Author POST rejects cross-origin writes"
    );
    let created_book = send(
        &app,
        request("POST", "/me/books", &create_form, &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(created_book.status(), StatusCode::SEE_OTHER);
    let book_path = created_book.headers()[header::LOCATION]
        .to_str()
        .expect("same-origin redirect")
        .to_owned();
    assert!(book_path.starts_with("/me/books/"));
    let replay = send(
        &app,
        request("POST", "/me/books", &create_form, &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::SEE_OTHER);
    assert_eq!(replay.headers()[header::LOCATION], book_path);
    let altered_form = create_form.replace("target_language=ru", "target_language=en");
    assert_eq!(
        send(
            &app,
            request("POST", "/me/books", &altered_form, &author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "replay with altered payload must not mutate the original book"
    );
    let book_page = send(&app, request("GET", &book_path, "", &session, ORIGIN)).await;
    assert_eq!(book_page.status(), StatusCode::OK);
    let authored_html = body_of(book_page).await;
    assert!(authored_html.contains("&lt;script&gt;"));
    assert!(!authored_html.contains("<script>"));
    let owner_index = send(&app, request("GET", "/me/books", "", &session, ORIGIN)).await;
    assert_eq!(owner_index.status(), StatusCode::OK);
    let index_html = body_of(owner_index).await;
    assert!(index_html.contains(&format!("href=\"{book_path}\"")));
    assert!(!index_html.contains("<script>"));
    let anon_detail = send(&app, request("GET", &book_path, "", "", ORIGIN)).await;
    assert_eq!(anon_detail.status(), StatusCode::SEE_OTHER);
    assert_eq!(anon_detail.headers()[header::LOCATION], "/login");

    // B-03/B-04: real browser writer journey with CSRF, immutable chapter and
    // deliberate release. The anonymous catalog does not leak the draft.
    let book_id = book_path
        .trim_start_matches("/me/books/")
        .parse::<i64>()
        .expect("book ID");
    assert_eq!(
        send(
            &app,
            request("GET", &format!("/books/{book_id}"), "", "", ORIGIN)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let chapter_form_path = format!("/me/books/{book_id}/chapters/new");
    assert_eq!(
        send(&app, request("GET", &chapter_form_path, "", "", ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let chapter_page = send(
        &app,
        request("GET", &chapter_form_path, "", &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(chapter_page.status(), StatusCode::OK);
    let chapter_html = body_of(chapter_page).await;
    let chapter_csrf = csrf_token(&chapter_html);
    let chapter_key = chapter_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .expect("chapter idempotency key")
        .split('"')
        .next()
        .unwrap();
    let chapter_payload = format!(
        "number=1&title=%D0%93%D0%BB%D0%B0%D0%B2%D0%B0&body=%23%20Real%20chapter&command_key={chapter_key}&csrf_token={chapter_csrf}"
    );
    let chapter_path = format!("/me/books/{book_id}/chapters");
    assert_eq!(
        send(
            &app,
            request("POST", &chapter_path, &chapter_payload, &session, ORIGIN)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "Chapter creation requires CSRF cookie"
    );
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                &chapter_path,
                &chapter_payload,
                &author_cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "Chapter writes reject other origins"
    );
    let chapter_created = send(
        &app,
        request(
            "POST",
            &chapter_path,
            &chapter_payload,
            &author_cookies,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(chapter_created.status(), StatusCode::SEE_OTHER);
    assert_eq!(chapter_created.headers()[header::LOCATION], book_path);
    // Retrying the exact command returns the same chapter, not a duplicate.
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                &chapter_path,
                &chapter_payload,
                &author_cookies,
                ORIGIN
            )
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let altered = chapter_payload.replace("number=1", "number=2");
    assert_eq!(
        send(
            &app,
            request("POST", &chapter_path, &altered, &author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let private_book = send(
        &app,
        request("GET", &book_path, "", &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(private_book.status(), StatusCode::OK);
    let author_book_html = body_of(private_book).await;
    assert!(author_book_html.contains("Опубликовать книгу"));
    assert!(author_book_html.contains("Глава"));
    let edit_path = b04_cases::draft_edit_case(
        &app,
        book_id,
        &book_path,
        &session,
        &author_cookies,
        &author_book_html,
    )
    .await;
    assert_eq!(
        send(
            &app,
            request("GET", &format!("/books/{book_id}"), "", "", ORIGIN)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // The owner draft has metadata, but anonymous discovery must not see it.
    let before = send(
        &app,
        request(
            "GET",
            "/api/v1/search/books?q=Alice&genre=fantasy",
            "",
            "",
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(before.status(), StatusCode::OK);
    assert!(!body_of(before)
        .await
        .contains(&format!(r#""id":"{book_id}""#)));
    let publish_path = format!("/me/books/{book_id}/publish");
    let publish_payload = format!("csrf_token={chapter_csrf}");
    assert_eq!(
        send(
            &app,
            request("POST", &publish_path, &publish_payload, &session, ORIGIN)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let released = send(
        &app,
        request(
            "POST",
            &publish_path,
            &publish_payload,
            &author_cookies,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(released.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        released.headers()[header::LOCATION],
        format!("/books/{book_id}")
    );
    let public_book = send(
        &app,
        request("GET", &format!("/books/{book_id}"), "", "", ORIGIN),
    )
    .await;
    assert_eq!(public_book.status(), StatusCode::OK);
    let public_html = body_of(public_book).await;
    assert!(public_html.contains("Глава"));
    // H-web-11: the actual author browser POST persists a queued image.
    // The same G worker processes it and real cover URLs resolve on all pages.
    h_cover_cases::upload_and_show(&app, &cover_store, book_id, &author_cookies, author_csrf).await;
    let discovered = send(
        &app,
        request(
            "GET",
            "/api/v1/search/books?q=Alice&genre=fantasy",
            "",
            "",
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(discovered.status(), StatusCode::OK);
    let found = body_of(discovered).await;
    let name_only = body_of(
        send(
            &app,
            request("GET", "/api/v1/search/books?q=Alice", "", "", ORIGIN),
        )
        .await,
    )
    .await;
    let genre_only = body_of(
        send(
            &app,
            request("GET", "/api/v1/search/books?genre=fantasy", "", "", ORIGIN),
        )
        .await,
    )
    .await;
    assert!(
        found.contains(&format!(r#""id":"{book_id}""#)),
        "book_id={book_id}; combined={found}; name_only={name_only}; genre_only={genre_only}"
    );
    assert!(found.contains("Alice Voyager"));
    let gallery = send(
        &app,
        request("GET", "/catalog?genre=fantasy", "", "", ORIGIN),
    )
    .await;
    assert_eq!(gallery.status(), StatusCode::OK);
    assert!(body_of(gallery).await.contains("Alice Voyager"));
    let public_chapter_id = public_html
        .split("href=\"/reader/")
        .nth(1)
        .expect("public chapter reader URL")
        .split('"')
        .next()
        .expect("chapter ID");
    let new_reader = send(
        &app,
        request(
            "GET",
            &format!("/reader/{public_chapter_id}"),
            "",
            "",
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(new_reader.status(), StatusCode::OK);
    assert!(
        body_of(new_reader).await.contains("Updated chapter"),
        "Published Markdown is readable through real PostgreSQL"
    );
    let published_detail = send(&app, request("GET", &book_path, "", &session, ORIGIN)).await;
    let published_html = body_of(published_detail).await;
    assert!(published_html.contains("Опубликована"));
    assert!(published_html.contains(&format!("href=\"/books/{book_id}\"")));
    assert_eq!(
        send(
            &app,
            request("GET", &edit_path, "", &author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "published chapter text cannot be edited as a draft"
    );
    // B-06: after the first release, new chapters stay hidden until an
    // explicit owner confirmation of the current immutable version.
    b06_cases::publish_second_chapter(&app, book_id, &book_path, &session, &author_cookies).await;
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                &publish_path,
                &publish_payload,
                &author_cookies,
                ORIGIN
            )
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "Publishing the same book twice is safe"
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
    // C-08: a book bookmark is independent of the stored reading position.
    let bookmark_prompt = send(
        &app,
        request("GET", "/me/library/add/1", "", &session, ORIGIN),
    )
    .await;
    assert_eq!(bookmark_prompt.status(), StatusCode::OK);
    let bookmark_cookie = cookie(&bookmark_prompt, "tor-dev-csrf=");
    let prompt_html = body_of(bookmark_prompt).await;
    let token = csrf_token(&prompt_html);
    let save_book = format!("csrf_token={token}");
    let saved_cookies = format!("{session}; {bookmark_cookie}");
    assert_eq!(
        send(
            &app,
            request("POST", "/api/v1/me/library/1", &save_book, &session, ORIGIN)
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
                "/api/v1/me/library/1",
                &save_book,
                &saved_cookies,
                "https://evil.example"
            )
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
                "/api/v1/me/library/2",
                &save_book,
                &saved_cookies,
                ORIGIN
            )
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        assert_eq!(
            send(
                &app,
                request(
                    "POST",
                    "/api/v1/me/library/1",
                    &save_book,
                    &saved_cookies,
                    ORIGIN
                )
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
    }
    let bookmarks = body_of(
        send(
            &app,
            request("GET", "/api/v1/me/library", "", &session, ORIGIN),
        )
        .await,
    )
    .await;
    assert!(bookmarks.contains("\"book_id\":\"1\""));
    assert_eq!(bookmarks.matches("\"book_id\"").count(), 1);
    assert!(!bookmarks.contains("PRIVATE-PAID-MARKER"));
    let saved_html =
        body_of(send(&app, request("GET", "/me/library", "", &session, ORIGIN)).await).await;
    assert!(saved_html.contains("Убрать из библиотеки"));
    assert_eq!(
        send(
            &app,
            request(
                "DELETE",
                "/api/v1/me/library/1",
                &save_book,
                &saved_cookies,
                ORIGIN
            )
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let after_remove = body_of(
        send(
            &app,
            request("GET", "/api/v1/me/library", "", &session, ORIGIN),
        )
        .await,
    )
    .await;
    assert!(after_remove.contains("\"items\":[]"));
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
    // C-09: JSON progress persists across independent client sessions, while
    // stale and concurrent requests cannot move the cursor backwards.
    let csrf_cookies = format!("{session}; {new_csrf_cookie}");
    c11_cases::reader_and_progress_share_access_policy(&app, &session, &csrf_cookies, csrf).await;
    let read_free = send(
        &app,
        json_request(
            "GET",
            "/api/v1/me/progress/free?chapter_id=101",
            "",
            &session,
            ORIGIN,
            "",
        ),
    )
    .await;
    assert_eq!(read_free.status(), StatusCode::OK);
    assert_eq!(read_free.headers()[header::CACHE_CONTROL], "no-store");
    let free_state = body_of(read_free).await;
    assert!(free_state.contains("\"version_id\":\"201\""));
    assert!(free_state.contains("\"revision\":1"));

    let fresh_place = r#"{"chapter_id":"101","version_id":"201","block_id":1,"char_offset":1,"expected_revision":1}"#;
    assert_eq!(
        send(
            &app,
            json_request(
                "PUT",
                "/api/v1/me/progress/free",
                fresh_place,
                &session,
                ORIGIN,
                csrf
            ),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "JSON write requires the matching CSRF cookie"
    );
    assert_eq!(
        send(
            &app,
            json_request(
                "PUT",
                "/api/v1/me/progress/free",
                fresh_place,
                &csrf_cookies,
                "https://evil.example",
                csrf,
            ),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "cross-site JSON write rejected"
    );
    let sync_saved = send(
        &app,
        json_request(
            "PUT",
            "/api/v1/me/progress/free",
            fresh_place,
            &csrf_cookies,
            ORIGIN,
            csrf,
        ),
    )
    .await;
    assert_eq!(sync_saved.status(), StatusCode::OK);
    let receipt = body_of(sync_saved).await;
    assert!(receipt.contains("\"revision\":2"));
    assert!(receipt.contains("\"char_offset\":1"));
    assert!(!receipt.contains("Первая строка"));
    assert_eq!(
        send(
            &app,
            json_request(
                "PUT",
                "/api/v1/me/progress/free",
                fresh_place,
                &csrf_cookies,
                ORIGIN,
                csrf,
            ),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let restored = body_of(
        send(
            &app,
            json_request(
                "GET",
                "/api/v1/me/progress/free?chapter_id=101",
                "",
                &session,
                ORIGIN,
                "",
            ),
        )
        .await,
    )
    .await;
    assert!(restored.contains("\"revision\":2"));
    assert!(restored.contains("\"char_offset\":1"));
    assert_eq!(
        send(
            &app,
            json_request(
                "GET",
                "/api/v1/me/progress/free?chapter_id=103",
                "",
                &session,
                ORIGIN,
                "",
            ),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "paid text is invisible through the free progress endpoint"
    );

    // A logged-in user cannot claim an invented purchase or see paid text.
    let paid_query =
        "/api/v1/me/progress/purchased?purchase_id=93840&chapter_id=93820&version_id=93831";
    assert_eq!(
        send(
            &app,
            json_request("GET", paid_query, "", &session, ORIGIN, ""),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let false_purchase = r#"{"purchase_id":"93840","chapter_id":"93820","version_id":"93831","block_id":1,"char_offset":2,"expected_revision":0}"#;
    assert_eq!(
        send(
            &app,
            json_request(
                "PUT",
                "/api/v1/me/progress/purchased",
                false_purchase,
                &csrf_cookies,
                ORIGIN,
                csrf,
            ),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let library_after = send(&app, request("GET", "/me/library", "", &session, ORIGIN)).await;
    let after = body_of(library_after).await;
    assert!(after.contains("href=\"/read/101#p-1\""));
    c11_cases::reopened_reader_has_saved_anchor(&app, &session).await;
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

    h_account_cases::duplicate_registration(&app, &form, &csrf_cookie).await;
    let login_page = send(&app, request("GET", "/login", "", "", ORIGIN)).await;
    let login_csrf_cookie = cookie(&login_page, "tor-dev-csrf=");
    let login_html = body_of(login_page).await;
    let login_csrf = csrf_token(&login_html);
    let bad_login =
        format!("email=tor_ci_account%40example.com&password=badpass&csrf_token={login_csrf}");
    h_account_cases::rejected_login(&app, &bad_login, &login_csrf_cookie).await;
    h_account_cases::seed_exhausted_proxy_counter(&lookup).await;
    let good_login = format!("email=tor_ci_account%40example.com&password=LongSecurePassword2026&csrf_token={login_csrf}");
    let logged = send(
        &app,
        request("POST", "/login", &good_login, &login_csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(logged.status(), StatusCode::SEE_OTHER);
    c11_cases::resume_after_login(&app, &logged).await;
    let second_session = cookie(&logged, "tor-dev-session=");
    c11_cases::second_session_reads_saved_progress(&app, &second_session).await;
    c11_cases::stale_second_session_cannot_overwrite(&app, &second_session).await;
    c11_cases::concurrent_sessions_one_progress_winner(&app, &second_session).await;
    c11_cases::reader_v2_html_flow(&app, &second_session, public_chapter_id).await;
    eprintln!("auth_pg checkpoint: start C06 paid journey");
    tokio::time::timeout(
        std::time::Duration::from_secs(90),
        c06_cases::purchased_version_http_journey(&app, &session, &second_session),
    )
    .await
    .expect("C06 paid HTTP journey exceeded 90 seconds");

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
