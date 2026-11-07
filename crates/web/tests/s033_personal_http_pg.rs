//! S033: session-owner HTTP vertical through disposable PostgreSQL; no shared access.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{net::SocketAddr, time::{SystemTime, UNIX_EPOCH};
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn request(method: &str, path: &str, cookies: &str, form: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .expect("HTTP request");
    request.extensions_mut().insert(axum::extract::ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40255))));
    request
}
async fn send(app: &Router, method: &str, path: &str, cookies: &str, form: &str) -> axum::response::Response {
    app.clone().oneshot(request(method, path, cookies, form)).await.expect("HTTP response")
}
async fn body(resp: axum::response::Response) -> String {
    let bytes = to_bytes(resp.into_body(), 2 * 1024 * 1024).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("UTF8")
}
fn csrf(html: &str) -> String {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1).expect("CSRF form field")
        .split('"').next().expect("value").to_owned()
}
fn cookie(reply: &axum::response::Response, prefix: &str) -> String {
    reply.headers().get_all(header::SET_COOKIE).iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(prefix))
        .expect("cookie").split(';').next().expect("pair").to_owned()
}
async fn signup(app: &Router, email: &str) -> String {
    let page = send(app, "GET", "/register", "", "").await;
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let value = csrf(&body(page).await);
    let form = format!(
        "email={}&display_name=PrivateOwner&password=LongSecurePassword2026&csrf_token={value}",
        email.replace('@', "%40")
    );
    let created = send(app, "POST", "/register", &csrf_cookie, &form).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    cookie(&created, "tor-dev-session=")
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires migrated, disposable PostgreSQL tor_ci"]
async fn s033_http_owner_only_create_edit_read_and_deny_all_cross_user_paths() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test DB");
    assert!(url.ends_with("/tor_ci"), "production forbidden");
    let catalog = PgCatalog::connect(&url).await.expect("connected app role");
    let accounts = PgAccounts::from_pool(catalog.pool());
    let reader = PgReader::new(catalog.pool());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog, reader, accounts, false, ORIGIN.to_owned(),
    );
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let owner = signup(&app, &format!("s033-owner-{stamp}@example.invalid")).await;
    let other = signup(&app, &format!("s033-other-{stamp}@example.invalid")).await;
    let guest = send(&app, "GET", "/personal-books", "", "").await;
    assert_eq!(guest.status(), StatusCode::SEE_OTHER);
    assert_eq!(guest.headers()[header::CACHE_CONTROL], "no-store");
    let home = send(&app, "GET", "/me", &owner, "").await;
    assert!(body(home).await.contains("href=\"/personal-books\""));

    let new = send(&app, "GET", "/personal-books/new", &owner, "").await;
    assert_eq!(new.headers()["x-robots-tag"], "noindex, nofollow, noarchive");
    let csrf_cookie = cookie(&new, "tor-dev-csrf=");
    let csrf = csrf(&body(new).await);
    let owner_cookies = format!("{owner}; {csrf_cookie}");

    let injected = format!("title=%3Cscript%3E&description=private&csrf_token={csrf}");
    let wrong_origin = send(&app, "POST", "/personal-books/new", &owner, &injected).await;
    assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN, "no CSRF cookie");
    let created = send(&app, "POST", "/personal-books/new", &owner_cookies, &injected).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let book_path = created.headers()[header::LOCATION].to_str().unwrap().to_owned();
    let own = send(&app, "GET", &book_path, &owner_cookies, "").await;
    assert_eq!(own.headers()[header::CACHE_CONTROL], "no-store");
    let html = body(own).await;
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script></h1>"));

    for path in [&book_path, &format!("{book_path}/edit"), &format!("{book_path}/chapters/new")] {
        assert_eq!(send(&app, "GET", path, &other, "").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(send(&app, "GET", path, "", "").await.status(), StatusCode::SEE_OTHER);
    }
    let foreign_form = format!("title=Hacked&description=x&csrf_token={csrf}");
    assert_eq!(send(&app, "POST", &format!("{book_path}/edit"), &owner, &foreign_form).await.status(), StatusCode::FORBIDDEN);
    let edited = send(&app, "POST", &format!("{book_path}/edit"), &owner_cookies,
                      &format!("title=Revised&description=Mine&csrf_token={csrf}")).await;
    assert_eq!(edited.status(), StatusCode::SEE_OTHER);
    let new_chapter_path = format!("{book_path}/chapters/new");
    let chapter_page = send(&app, "GET", &new_chapter_path, &owner_cookies, "").await;
    let chapter_csrf = csrf(&body(chapter_page).await);
    let chapter = send(&app, "POST", &new_chapter_path, &owner_cookies,
                      &format!("title=Private&content_markdown=SECRET_S033_%3Cimg%3E&csrf_token={chapter_csrf}")).await;
    assert_eq!(chapter.status(), StatusCode::SEE_OTHER);
    let chapter_path = chapter.headers()[header::LOCATION].to_str().unwrap().to_owned();
    let own_chapter = send(&app, "GET", &chapter_path, &owner, "").await;
    let html = body(own_chapter).await;
    assert!(html.contains("SECRET_S033_&lt;img&gt;"));
    assert!(!html.contains("<img>"));
    for path in [&chapter_path, &format!("{chapter_path}/edit")] {
        assert_eq!(send(&app, "GET", path, &other, "").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(send(&app, "GET", path, "", "").await.status(), StatusCode::SEE_OTHER);
    }
    assert_eq!(send(&app, "POST", &format!("{chapter_path}/edit"), &owner_cookies,
                &format!("title=Changed&content_markdown=CHANGED&csrf_token={csrf}")).await.status(),
               StatusCode::SEE_OTHER);
    assert!(body(send(&app, "GET", &chapter_path, &owner, "").await).await.contains("CHANGED"));
    let other_list = body(send(&app, "GET", "/personal-books", &other, "").await).await;
    assert!(!other_list.contains("Revised"));
    assert!(!body(send(&app, "GET", "/catalog", "", "").await).await.contains("Revised"));
    assert_eq!(send(&app, "POST", &new_chapter_path, &owner_cookies,
                    &format!("title=Intrusion&content_markdown=x&csrf_token=invalid")).await.status(), StatusCode::FORBIDDEN);
    // Personal books are in separate PG tables and never projected into public catalog.
}
