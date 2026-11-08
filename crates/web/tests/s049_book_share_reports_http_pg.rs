//! S049: recipient report -> administrator decision -> immutable audit over real HTTP.
//! Isolated PostgreSQL only; site moderator cannot read or decide complaints.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    response::Response,
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

fn pg(query: &str) -> String {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.contains("127.0.0.1:5432") && url.ends_with("/tor_ci"));
    let result = Command::new("psql")
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
        .expect("psql");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn request(method: &str, path: &str, cookie: &str, form: &str, origin: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .body(Body::from(form.to_owned()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 43141))));
    req
}
async fn send(app: &Router, method: &str, path: &str, cookie: &str, form: &str) -> Response {
    app.clone()
        .oneshot(request(method, path, cookie, form, ORIGIN))
        .await
        .unwrap()
}
async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
fn cookie(response: &Response, prefix: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|x| x.to_str().ok())
        .find(|v| v.starts_with(prefix))
        .expect("browser cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}
fn field(page: &str, name: &str) -> String {
    page.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .expect("CSRF form field")
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
async fn signup(app: &Router, email: &str, label: &str) -> String {
    let page = send(app, "GET", "/register", "", "").await;
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let csrf = field(&html(page).await, "csrf_token");
    let form = format!(
        "email={}&display_name={label}&password=LongSecurePassword2026&csrf_token={csrf}",
        email.replace('@', "%40")
    );
    let signed = send(app, "POST", "/register", &csrf_cookie, &form).await;
    assert_eq!(signed.status(), StatusCode::SEE_OTHER);
    cookie(&signed, "tor-dev-session=")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s049_admin_only_receives_share_reports_and_decision_is_once_with_audit() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
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
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sender_email = format!("s049-sender-{stamp}@example.invalid");
    let recipient_email = format!("s049-recipient-{stamp}@example.invalid");
    let admin_email = format!("s049-admin-{stamp}@example.invalid");
    let moderator_email = format!("s049-moderator-{stamp}@example.invalid");
    let sender = signup(&app, &sender_email, "Sender").await;
    let recipient = signup(&app, &recipient_email, "Recipient").await;
    let admin = signup(&app, &admin_email, "Admin").await;
    let moderator = signup(&app, &moderator_email, "Moderator").await;
    let sender_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='{sender_email}'"
    ))
    .parse()
    .unwrap();
    let recipient_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='{recipient_email}'"
    ))
    .parse()
    .unwrap();
    let admin_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='{admin_email}'"
    ))
    .parse()
    .unwrap();
    let moderator_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='{moderator_email}'"
    ))
    .parse()
    .unwrap();
    pg(&format!(
        "INSERT INTO account_roles(user_id,role)
         VALUES({admin_id},'administrator'),({moderator_id},'moderator')"
    ));
    let book: i64 = pg(&format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
         VALUES('ci:s049:{stamp}','ru','SafeBook','public','basic',{sender_id}) RETURNING id"
    ))
    .parse()
    .unwrap();
    let share: i64 = pg(&format!(
        "INSERT INTO social_book_shares(sender_user_id,recipient_user_id,book_id,message,command_key)
         VALUES({sender_id},{recipient_id},{book},'Message <script>unsafe()</script>','{}')
         RETURNING id",
        "A".repeat(43)
    ))
    .parse()
    .unwrap();
    let admin_path = "/admin/recommendation-reports";
    assert_eq!(
        send(&app, "GET", admin_path, "", "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, "GET", admin_path, &sender, "").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "GET", admin_path, &moderator, "").await.status(),
        StatusCode::FORBIDDEN,
        "site moderator reviews Book registrations, not reports"
    );
    // The user reports a real delivered recommendation through S017 HTTP.
    let inbox = send(&app, "GET", "/library/shared", &recipient, "").await;
    assert_eq!(inbox.status(), StatusCode::OK);
    let recipient_csrf_cookie = cookie(&inbox, "tor-dev-csrf=");
    let recipient_csrf = field(&html(inbox).await, "csrf_token");
    let recipient_cookies = format!("{recipient}; {recipient_csrf_cookie}");
    let report_form =
        format!("action=report&reason=spam&details=Repeated&csrf_token={recipient_csrf}");
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/library/shared/{share}/action"),
            &recipient_cookies,
            &report_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let queue = send(&app, "GET", admin_path, &admin, "").await;
    assert_eq!(queue.status(), StatusCode::OK);
    assert_eq!(queue.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        queue.headers()["x-robots-tag"],
        "noindex, nofollow, noarchive"
    );
    let admin_csrf_cookie = cookie(&queue, "tor-dev-csrf=");
    let queue_html = html(queue).await;
    assert!(queue_html.contains(&format!("Жалоба #{share}")));
    assert!(queue_html.contains("Repeated"));
    assert!(queue_html.contains("&lt;script&gt;unsafe()&lt;/script&gt;"));
    assert!(!queue_html.contains("<script>unsafe()"));
    let admin_csrf = field(&queue_html, "csrf_token");
    let admin_cookies = format!("{admin}; {admin_csrf_cookie}");
    let decide_path = format!("{admin_path}/{share}/decision");
    let choice = format!("decision=reviewed&note=Confirmed&csrf_token={admin_csrf}");
    assert_eq!(
        send(&app, "POST", &decide_path, &admin, &choice)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                &decide_path,
                &admin_cookies,
                &choice,
                "https://evil.invalid"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let invalid = format!("decision=block-book&note=No&csrf_token={admin_csrf}");
    assert_eq!(
        send(&app, "POST", &decide_path, &admin_cookies, &invalid)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    // A prepared form must not survive a live administrator-role revocation.
    pg(&format!(
        "UPDATE account_roles SET is_active=false WHERE user_id={admin_id} AND role='administrator'"
    ));
    assert_eq!(
        send(&app, "POST", &decide_path, &admin_cookies, &choice)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    pg(&format!(
        "UPDATE account_roles SET is_active=true WHERE user_id={admin_id} AND role='administrator'"
    ));
    let (left, right) = tokio::join!(
        send(&app, "POST", &decide_path, &admin_cookies, &choice),
        send(&app, "POST", &decide_path, &admin_cookies, &choice)
    );
    let mut results = [left.status().as_u16(), right.status().as_u16()];
    results.sort();
    assert_eq!(
        results,
        [
            StatusCode::SEE_OTHER.as_u16(),
            StatusCode::CONFLICT.as_u16()
        ]
    );
    assert_eq!(
        pg(&format!(
            "SELECT state FROM social_book_share_reports WHERE share_id={share}"
        )),
        "reviewed"
    );
    assert_eq!(
        pg(&format!(
            "SELECT count(*) FROM social_book_share_report_reviews
             WHERE share_id={share} AND reviewer_user_id={admin_id}
             AND decision='reviewed' AND note='Confirmed'"
        )),
        "1"
    );
    assert_eq!(
        pg(&format!("SELECT admin_hold FROM books WHERE id={book}")),
        "f",
        "a share complaint must not silently block a published Book"
    );
    assert_eq!(
        pg(&format!(
            "SELECT count(*) FROM library_purchased_chapters
             WHERE user_id IN ({recipient_id},{sender_id})"
        )),
        "0"
    );
    assert!(
        !html(send(&app, "GET", admin_path, &admin_cookies, "").await)
            .await
            .contains(&format!("Жалоба #{share}"))
    );
    assert_eq!(
        send(&app, "POST", &decide_path, &admin_cookies, &choice)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    // The PG workflow runs later A-04/A-05 suites in the same disposable DB.
    // A still-live test administrator would invalidate their last-admin
    // invariant. Leave immutable S049 evidence intact, but deactivate our
    // fixture's administrative role before the next suite.
    pg(&format!(
        "UPDATE account_roles SET is_active=false
         WHERE user_id={admin_id} AND role='administrator'"
    ));
}
