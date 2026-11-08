//! S016/S017: real HTTP share-inbox scenario on isolated PostgreSQL.
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
async fn html(r: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(r.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
fn field(page: &str, name: &str) -> String {
    page.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .unwrap_or_else(|| panic!("missing form field {name}"))
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
        .filter_map(|value| value.to_str().ok())
        .find(|s| s.starts_with(name))
        .expect("cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}
async fn signup(app: &Router, email: &str, label: &str) -> String {
    let first = send(app, "GET", "/register", "", "").await;
    let csrf_cookie = cookie(&first, "tor-dev-csrf=");
    let token = field(&html(first).await, "csrf_token");
    let form = format!(
        "email={}&display_name={label}&password=LongSecurePassword2026&csrf_token={token}",
        email.replace('@', "%40")
    );
    let result = send(app, "POST", "/register", &csrf_cookie, &form).await;
    assert_eq!(result.status(), StatusCode::SEE_OTHER);
    cookie(&result, "tor-dev-session=")
}
fn pg(query: &str) -> String {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    let out = Command::new("psql")
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
        .expect("psql");
    assert!(
        out.status.success(),
        "PG: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s016_s017_http_friend_share_csrf_replay_inbox_hide_report_and_mute() {
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
    let sender = signup(
        &app,
        &format!("sh-s-{stamp}@example.invalid"),
        "FriendSender",
    )
    .await;
    let receiver = signup(
        &app,
        &format!("sh-r-{stamp}@example.invalid"),
        "FriendReceiver",
    )
    .await;
    let sender_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='sh-s-{stamp}@example.invalid'"
    ))
    .parse()
    .unwrap();
    let recipient_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='sh-r-{stamp}@example.invalid'"
    ))
    .parse()
    .unwrap();
    let book:i64=pg(&format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
         VALUES('ci:shared:{stamp}','ru','SharedBook{stamp}','public','basic',{sender_id}) RETURNING id"
    )).parse().unwrap();
    let share_path = format!("/books/{book}/share");
    let library_html = html(send(&app, "GET", "/me/library", &receiver, "").await).await;
    assert!(library_html.contains("href=\"/library/shared\""));
    let first = send(&app, "GET", &share_path, &sender, "").await;
    assert_eq!(first.status(), StatusCode::OK);
    let csrf_cookie = cookie(&first, "tor-dev-csrf=");
    let initial = html(first).await;
    assert!(initial.contains("Нет доступных друзей"));
    pg(&format!(
        "INSERT INTO social_friendships(user_low,user_high,requester_id,status,action_user_id,accepted_at)
         VALUES({},{},{sender_id},'accepted',{recipient_id},clock_timestamp())",
        sender_id.min(recipient_id),sender_id.max(recipient_id)
    ));
    let page = html(
        send(
            &app,
            "GET",
            &share_path,
            &format!("{sender}; {csrf_cookie}"),
            "",
        )
        .await,
    )
    .await;
    let csrf = field(&page, "csrf_token");
    let key = field(&page, "command_key");
    assert!(page.contains(&format!(r#"value="{recipient_id}""#)));
    let form = format!(
        "recipient_user_id={recipient_id}&message=Recommended&command_key={key}&csrf_token={csrf}"
    );
    let sender_cookies = format!("{sender}; {csrf_cookie}");
    assert_eq!(
        send(&app, "POST", &share_path, &sender, &form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                &share_path,
                &sender_cookies,
                &form,
                "https://evil.invalid"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &share_path, &csrf_cookie, &form)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let sent = send(&app, "POST", &share_path, &sender_cookies, &form).await;
    assert_eq!(sent.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        sent.headers()[header::LOCATION],
        format!("{share_path}?sent=1")
    );
    assert!(html(
        send(
            &app,
            "GET",
            &format!("{share_path}?sent=1"),
            &sender_cookies,
            ""
        )
        .await
    )
    .await
    .contains("Рекомендация отправлена."));
    assert_eq!(
        send(&app, "POST", &share_path, &sender_cookies, &form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let count: i64 = pg(&format!(
        "SELECT COUNT(*) FROM social_book_shares WHERE sender_user_id={sender_id}
         AND recipient_user_id={recipient_id} AND book_id={book}"
    ))
    .parse()
    .unwrap();
    assert_eq!(count, 1);
    let share_id: i64 = pg(&format!(
        "SELECT id FROM social_book_shares WHERE sender_user_id={sender_id}
         AND recipient_user_id={recipient_id}"
    ))
    .parse()
    .unwrap();
    assert_eq!(
        send(&app, "GET", "/library/shared", "", "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        !html(send(&app, "GET", "/library/shared", &sender, "").await)
            .await
            .contains("SharedBook")
    );
    let received = send(&app, "GET", "/library/shared", &receiver, "").await;
    assert_eq!(received.status(), StatusCode::OK);
    assert_eq!(received.headers()[header::CACHE_CONTROL], "no-store");
    let recipient_cookie = cookie(&received, "tor-dev-csrf=");
    let received_page = html(received).await;
    assert!(received_page.contains(&format!("SharedBook{stamp}")));
    assert!(received_page.contains("FriendSender"));
    let recipient_csrf = field(&received_page, "csrf_token");
    let receiver_cookies = format!("{receiver}; {recipient_cookie}");
    let hide = format!("action=hide&reason=&details=&csrf_token={recipient_csrf}");
    let action_path = format!("/library/shared/{share_id}/action");
    // A stolen recipient CSRF must be rejected before the ownership guard.
    assert_eq!(
        send(&app, "POST", &action_path, &sender_cookies, &hide)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // With the sender's own valid CSRF, the recipient-only ACL still rejects.
    let own_csrf = format!("action=hide&reason=&details=&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", &action_path, &sender_cookies, &own_csrf)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "POST", &action_path, &receiver_cookies, &hide)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert!(
        !html(send(&app, "GET", "/library/shared", &receiver_cookies, "").await)
            .await
            .contains("SharedBook")
    );
    let new_key = "2".repeat(43);
    let second = format!(
        "recipient_user_id={recipient_id}&message=Again&command_key={new_key}&csrf_token={csrf}"
    );
    assert_eq!(
        send(&app, "POST", &share_path, &sender_cookies, &second)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let second_id: i64 = pg(&format!(
        "SELECT max(id) FROM social_book_shares WHERE recipient_user_id={recipient_id}
         AND sender_user_id={sender_id}"
    ))
    .parse()
    .unwrap();
    let report = format!("action=report&reason=spam&details=Annoying&csrf_token={recipient_csrf}");
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/library/shared/{second_id}/action"),
            &receiver_cookies,
            &report
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let reported: i64 = pg(&format!(
        "SELECT COUNT(*) FROM social_book_share_reports WHERE share_id={second_id}
         AND reason='spam' AND reporter_user_id={recipient_id}"
    ))
    .parse()
    .unwrap();
    assert_eq!(reported, 1);
    let third_key = "3".repeat(43);
    let third = format!(
        "recipient_user_id={recipient_id}&message=Third&command_key={third_key}&csrf_token={csrf}"
    );
    assert_eq!(
        send(&app, "POST", &share_path, &sender_cookies, &third)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let third_id: i64 = pg(&format!(
        "SELECT max(id) FROM social_book_shares WHERE recipient_user_id={recipient_id}
         AND sender_user_id={sender_id}"
    ))
    .parse()
    .unwrap();
    let mute = format!("action=mute&reason=&details=&csrf_token={recipient_csrf}");
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/library/shared/{third_id}/action"),
            &receiver_cookies,
            &mute
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let fourth_key = "4".repeat(43);
    let fourth = format!(
        "recipient_user_id={recipient_id}&message=Fourth&command_key={fourth_key}&csrf_token={csrf}"
    );
    assert_eq!(
        send(&app, "POST", &share_path, &sender_cookies, &fourth)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        pg(&format!(
            "SELECT COUNT(*) FROM library_purchased_chapters
                    WHERE user_id={recipient_id}"
        )),
        "0"
    );
}
