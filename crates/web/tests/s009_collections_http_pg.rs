//! S009: real browser-form HTTP entry points with a disposable PG database.
//! No shared or production database is permitted.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{
    net::SocketAddr,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn request(method: &str, path: &str, cookies: &str, form: &str, origin: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .expect("HTTP request");
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            40431,
        ))));
    request
}
async fn send(
    app: &Router,
    method: &str,
    path: &str,
    cookies: &str,
    form: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(method, path, cookies, form, ORIGIN))
        .await
        .expect("HTTP response")
}
async fn html(reply: axum::response::Response) -> String {
    let raw = to_bytes(reply.into_body(), 2 * 1024 * 1024).await.unwrap();
    String::from_utf8(raw.to_vec()).unwrap()
}
fn field(html: &str, name: &str) -> String {
    html.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .unwrap_or_else(|| panic!("missing HTML field {name}"))
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
fn cookie(reply: &axum::response::Response, name: &str) -> String {
    reply
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|line| line.starts_with(name))
        .expect("cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}
async fn signup(app: &Router, email: &str) -> String {
    let first = send(app, "GET", "/register", "", "").await;
    let csrf_cookie = cookie(&first, "tor-dev-csrf=");
    let csrf = field(&html(first).await, "csrf_token");
    let register = format!(
        "email={}&display_name=CollectionReader&password=LongSecurePassword2026&csrf_token={csrf}",
        email.replace('@', "%40")
    );
    let result = send(app, "POST", "/register", &csrf_cookie, &register).await;
    assert_eq!(result.status(), StatusCode::SEE_OTHER);
    cookie(&result, "tor-dev-session=")
}
fn database() -> String {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    url
}
fn pg(query: &str) -> String {
    let url = database();
    let output = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c lock_timeout=15000 -c statement_timeout=60000",
        )
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            &url,
            "--command",
            query,
        ])
        .output()
        .expect("psql in isolated CI");
    assert!(
        output.status.success(),
        "PG fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires migrated, disposable PostgreSQL tor_ci"]
async fn s009_collections_real_http_create_select_hide_publish_replay_and_delete() {
    let url = database();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool());
    let reader = PgReader::new(catalog.pool());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner = signup(&app, &format!("s009-owner-{unique}@example.invalid")).await;
    let other = signup(&app, &format!("s009-other-{unique}@example.invalid")).await;

    let guest_index = send(&app, "GET", "/collections", "", "").await;
    assert_eq!(guest_index.status(), StatusCode::OK);
    assert!(!html(guest_index).await.contains("name=\"request_key\""));

    let index = send(&app, "GET", "/collections", &owner, "").await;
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(index.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&index, "tor-dev-csrf=");
    let page = html(index).await;
    let key = field(&page, "request_key");
    let csrf = field(&page, "csrf_token");
    let cookies = format!("{owner}; {csrf_cookie}");
    let creation =
        format!("title=PrivateShelves&visibility=private&request_key={key}&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", "/collections", &owner, &creation)
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                "/collections",
                &cookies,
                &creation,
                "https://evil.invalid"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(&app, "POST", "/collections", &csrf_cookie, &creation)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
    );
    let made = send(&app, "POST", "/collections", &cookies, &creation).await;
    assert_eq!(made.status(), StatusCode::SEE_OTHER);
    let path = made.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(path.starts_with("/collections/"));
    assert_eq!(
        send(&app, "POST", "/collections", &cookies, &creation)
            .await
            .headers()[header::LOCATION],
        path.as_str(),
        "duplicate form must not create another collection",
    );
    assert_eq!(
        send(&app, "GET", &path, &other, "").await.status(),
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        send(&app, "GET", &path, "", "").await.status(),
        StatusCode::NOT_FOUND,
    );

    let title = format!("S009Book{unique}");
    let owner_id = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='s009-owner-{unique}@example.invalid'"
    ));
    let book: i64 = pg(&format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
         VALUES('ci:s009:{unique}','ru','{title}','public','basic',{owner_id}) RETURNING id"
    )).parse().unwrap();
    let search = html(
        send(
            &app,
            "GET",
            &format!("{path}?q=S009Book{unique}"),
            &cookies,
            "",
        )
        .await,
    )
    .await;
    assert!(search.contains(&format!(r#"<option value="{book}">"#)));
    assert!(
        !html(send(&app, "GET", &format!("{path}?q=S009Book{unique}"), "", "").await)
            .await
            .contains("name=\"book_id\"")
    );
    let invalid = format!("book_id=0&expected_revision=1&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", &format!("{path}/books"), &cookies, &invalid)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let add = format!("book_id={book}&expected_revision=1&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", &format!("{path}/books"), &cookies, &add)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "POST", &format!("{path}/books"), &cookies, &add)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert!(html(send(&app, "GET", &path, &cookies, "").await)
        .await
        .contains(&title));
    let after_add = html(
        send(
            &app,
            "GET",
            &format!("{path}?q=S009Book{unique}"),
            &cookies,
            "",
        )
        .await,
    )
    .await;
    assert!(!after_add.contains(&format!(r#"<option value="{book}">"#)));
    let stolen = format!("title=Hacked&visibility=public&expected_revision=2&csrf_token={csrf}");
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{path}/edit"),
            &format!("{other}; {csrf_cookie}"),
            &stolen
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let publish =
        format!("title=PublicShelves&visibility=public&expected_revision=2&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", &format!("{path}/edit"), &cookies, &publish)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let public_html = html(send(&app, "GET", &path, "", "").await).await;
    assert!(public_html.contains("PublicShelves"));
    assert!(public_html.contains(&title));
    assert!(
        !public_html.contains("name=\"expected_revision\""),
        "guest must never see owner controls"
    );
    assert!(html(send(&app, "GET", "/collections", "", "").await)
        .await
        .contains("PublicShelves"));

    // Public feedback, including CSRF, guest read, replay and HTML escaping.
    let other_page = send(&app, "GET", &path, &other, "").await;
    let other_csrf_cookie = cookie(&other_page, "tor-dev-csrf=");
    let other_html = html(other_page).await;
    let other_csrf = field(&other_html, "csrf_token");
    let other_key = field(&other_html, "request_key");
    let other_cookies = format!("{other}; {other_csrf_cookie}");
    let rating = format!("rating=5&csrf_token={other_csrf}");
    assert_eq!(
        send(&app, "POST", &format!("{path}/rate"), "", &rating)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{path}/rate"),
            &other_cookies,
            &rating
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(html(send(&app, "GET", &path, "", "").await)
        .await
        .contains("5.0 из 5"));

    let comment = format!(
        "body=%3Cscript%3Ealert%281%29%3C%2Fscript%3E&request_key={other_key}&csrf_token={other_csrf}"
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{path}/comments"),
            &other_cookies,
            &comment
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{path}/comments"),
            &other_cookies,
            &comment
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let public_comments = html(send(&app, "GET", &path, "", "").await).await;
    assert!(public_comments.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!public_comments.contains("<script>alert(1)</script>"));
    assert!(!public_comments.contains("Удалить комментарий"));
    let collection_id: i64 = path.trim_start_matches("/collections/").parse().unwrap();
    let comment_id: i64 = pg(&format!(
        "SELECT id FROM social_book_collection_comments
         WHERE collection_id={collection_id} AND deleted_at IS NULL"
    ))
    .parse()
    .unwrap();
    let remove = format!("csrf_token={csrf}");
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{path}/comments/{comment_id}/delete"),
            &cookies,
            &remove
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(!html(send(&app, "GET", &path, "", "").await)
        .await
        .contains("&lt;script&gt;"));

    // Two thousand Unicode characters must fit the bounded URL-encoded form.
    let next_html = html(send(&app, "GET", &path, &other_cookies, "").await).await;
    let fresh_key = field(&next_html, "request_key");
    let russian_form = format!(
        "body={}&request_key={fresh_key}&csrf_token={other_csrf}",
        "%D0%96".repeat(1500)
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{path}/comments"),
            &other_cookies,
            &russian_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );

    // Percent-encoded supplementary Unicode also fits the 2000-char policy.
    let emoji_html = html(send(&app, "GET", &path, &other_cookies, "").await).await;
    let emoji_key = field(&emoji_html, "request_key");
    let emoji_form = format!(
        "body={}&request_key={emoji_key}&csrf_token={other_csrf}",
        "%F0%9F%98%80".repeat(1500)
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{path}/comments"),
            &other_cookies,
            &emoji_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );

    pg(&format!(
        "UPDATE books SET visibility='draft' WHERE id={book}"
    ));
    assert!(!html(send(&app, "GET", &path, "", "").await)
        .await
        .contains(&title));
    let delete = format!("expected_revision=3&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", &format!("{path}/delete"), &cookies, &delete)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "GET", &path, &cookies, "").await.status(),
        StatusCode::NOT_FOUND,
    );
    assert!(!html(send(&app, "GET", "/collections", "", "").await)
        .await
        .contains("PublicShelves"));
}
