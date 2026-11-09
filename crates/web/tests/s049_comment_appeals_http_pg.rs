//! S049: an independently reviewed appeal against an explicit Book-comment hide.
//! Disposable tor_ci PostgreSQL only. No production or paid content is accessed.
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

fn pg(query: &str, success: bool) -> String {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
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
    assert_eq!(
        result.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn request(method: &str, path: &str, cookies: &str, form: &str, origin: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40492))));
    req
}

async fn send(app: &Router, method: &str, path: &str, cookies: &str, form: &str) -> Response {
    app.clone()
        .oneshot(request(method, path, cookies, form, ORIGIN))
        .await
        .unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn cookie(response: &Response, name: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .find(|h| h.starts_with(name))
        .expect("cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn field(page: &str, name: &str) -> String {
    page.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .expect("hidden field")
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

async fn signup(app: &Router, email: &str, name: &str) -> String {
    let page = send(app, "GET", "/register", "", "").await;
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let csrf = field(&body(page).await, "csrf_token");
    let post = format!(
        "email={}&display_name={name}&password=LongSecurePassword2026&csrf_token={csrf}",
        email.replace('@', "%40")
    );
    let res = send(app, "POST", "/register", &csrf_cookie, &post).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    cookie(&res, "tor-dev-session=")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s049_comment_hide_appeal_requires_independent_admin_and_immutable_audit() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(catalog.pool()),
        PgAccounts::from_pool(catalog.pool()),
        false,
        ORIGIN.to_owned(),
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let author_mail = format!("s049-appeal-author-{stamp}@example.invalid");
    let reporter_mail = format!("s049-appeal-reporter-{stamp}@example.invalid");
    let first_mail = format!("s049-appeal-first-{stamp}@example.invalid");
    let second_mail = format!("s049-appeal-second-{stamp}@example.invalid");
    let mod_mail = format!("s049-appeal-mod-{stamp}@example.invalid");
    let author = signup(&app, &author_mail, "AppealAuthor").await;
    let reporter = signup(&app, &reporter_mail, "AppealReporter").await;
    let first = signup(&app, &first_mail, "AppealFirst").await;
    let second = signup(&app, &second_mail, "AppealSecond").await;
    let moderator = signup(&app, &mod_mail, "AppealModerator").await;
    let author_id: i64 = pg(
        &format!("SELECT id FROM users WHERE email_normalized='{author_mail}'"),
        true,
    )
    .parse()
    .unwrap();
    let first_id: i64 = pg(
        &format!("SELECT id FROM users WHERE email_normalized='{first_mail}'"),
        true,
    )
    .parse()
    .unwrap();
    let second_id: i64 = pg(
        &format!("SELECT id FROM users WHERE email_normalized='{second_mail}'"),
        true,
    )
    .parse()
    .unwrap();
    let mod_id: i64 = pg(
        &format!("SELECT id FROM users WHERE email_normalized='{mod_mail}'"),
        true,
    )
    .parse()
    .unwrap();
    pg(
        &format!(
            "INSERT INTO account_roles(user_id,role)
         VALUES({first_id},'administrator'),({second_id},'administrator'),({mod_id},'moderator')"
        ),
        true,
    );
    let book: i64 = pg(&format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
         VALUES('ci:s049:appeal:{stamp}','ru','AppealBook','public','basic',{author_id})
         RETURNING id"
    ), true).parse().unwrap();
    let comment: i64 = pg(
        &format!(
            "INSERT INTO discussion_comments
         (author_user_id,book_id,body,visibility,command_key,command_hash)
         VALUES({author_id},{book},'Moderated public Book comment','visible',
                '{}',decode(repeat('03',32),'hex')) RETURNING id",
            "C".repeat(43)
        ),
        true,
    )
    .parse()
    .unwrap();
    // The normal report + explicit administrator action must happen first.
    let report_path = format!("/books/{book}/report?comment_id={comment}");
    let report_page = send(&app, "GET", &report_path, &reporter, "").await;
    assert_eq!(report_page.status(), StatusCode::OK);
    let report_cookie = cookie(&report_page, "tor-dev-csrf=");
    let report_html = body(report_page).await;
    let report_csrf = field(&report_html, "csrf_token");
    let report_key = field(&report_html, "command_key");
    let report_post = format!(
        "comment_id={comment}&reason=abuse&details=Complaint&csrf_token={report_csrf}&command_key={report_key}"
    );
    let res = send(
        &app,
        "POST",
        &format!("/books/{book}/report"),
        &format!("{reporter}; {report_cookie}"),
        &report_post,
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let report: i64 = pg(
        &format!(
        "SELECT id FROM social_book_content_reports WHERE comment_id={comment} AND state='pending'"
    ),
        true,
    )
    .parse()
    .unwrap();
    let first_page = send(&app, "GET", "/admin/book-reports", &first, "").await;
    let first_cookie = cookie(&first_page, "tor-dev-csrf=");
    let first_csrf = field(&body(first_page).await, "csrf_token");
    let hide = format!("decision=hide_comment&note=Hide+for+review&csrf_token={first_csrf}");
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/admin/book-reports/{report}/decision"),
            &format!("{first}; {first_cookie}"),
            &hide
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &format!("SELECT visibility FROM discussion_comments WHERE id={comment}"),
            true
        ),
        "hidden"
    );
    let event: i64 = pg(
        &format!(
            "SELECT id FROM social_book_content_review_events
         WHERE report_id={report} AND moderation_action='hide_book_comment'"
        ),
        true,
    )
    .parse()
    .unwrap();

    let author_path = "/me/comment-appeals";
    assert_eq!(
        send(&app, "GET", author_path, "", "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    let stranger_page = body(send(&app, "GET", author_path, &reporter, "").await).await;
    assert!(!stranger_page.contains(&format!("решение #{event}")));
    let appeal_page = send(&app, "GET", author_path, &author, "").await;
    assert_eq!(appeal_page.status(), StatusCode::OK);
    assert_eq!(appeal_page.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        appeal_page.headers()["x-robots-tag"],
        "noindex, nofollow, noarchive"
    );
    let appeal_cookie = cookie(&appeal_page, "tor-dev-csrf=");
    let appeal_html = body(appeal_page).await;
    assert!(appeal_html.contains(&format!("решение #{event}")));
    assert!(!appeal_html.contains("Moderated public Book comment"));
    let appeal_csrf = field(&appeal_html, "csrf_token");
    let appeal_key = field(&appeal_html, "command_key");
    let appeal_post = format!(
        "reason=Please+reconsider+%3Cscript%3Ebad%3C%2Fscript%3E\
         &csrf_token={appeal_csrf}&command_key={appeal_key}"
    );
    let appeal_path = format!("{author_path}/{event}");
    let appeal_cookies = format!("{author}; {appeal_cookie}");
    assert_eq!(
        send(&app, "POST", &appeal_path, &author, &appeal_post)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                &appeal_path,
                &appeal_cookies,
                &appeal_post,
                "https://evil.invalid"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &appeal_path,
            &format!("{reporter}; {report_cookie}"),
            &appeal_post.replace(&appeal_csrf, &report_csrf)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "POST", &appeal_path, &appeal_cookies, &appeal_post)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "POST", &appeal_path, &appeal_cookies, &appeal_post)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &appeal_path,
            &appeal_cookies,
            &appeal_post.replace(&appeal_key, &"Z".repeat(43))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let appeal: i64 = pg(
        &format!(
            "SELECT id FROM social_book_comment_appeals
         WHERE hide_event_id={event} AND author_user_id={author_id} AND state='pending'"
        ),
        true,
    )
    .parse()
    .unwrap();
    pg(
        &format!("UPDATE social_book_comment_appeals SET reason='tampered' WHERE id={appeal}"),
        false,
    );

    let queue = "/admin/book-comment-appeals";
    assert_eq!(
        send(&app, "GET", queue, "", "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, "GET", queue, &author, "").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "GET", queue, &moderator, "").await.status(),
        StatusCode::FORBIDDEN
    );
    let admin_page = send(&app, "GET", queue, &second, "").await;
    assert_eq!(admin_page.status(), StatusCode::OK);
    let admin_cookie = cookie(&admin_page, "tor-dev-csrf=");
    let admin_html = body(admin_page).await;
    assert!(admin_html.contains(&format!("Апелляция #{appeal}")));
    assert!(admin_html.contains("&lt;script&gt;bad&lt;/script&gt;"));
    assert!(!admin_html.contains("<script>bad</script>"));
    assert!(!admin_html.contains("Moderated public Book comment"));
    let admin_csrf = field(&admin_html, "csrf_token");
    let decision = format!("decision=restored&note=Second+review&csrf_token={admin_csrf}");
    let decision_path = format!("{queue}/{appeal}/decision");
    let moderator_page = send(&app, "GET", "/me", &moderator, "").await;
    assert_eq!(moderator_page.status(), StatusCode::OK);
    let moderator_cookie = cookie(&moderator_page, "tor-dev-csrf=");
    let moderator_csrf = field(&body(moderator_page).await, "csrf_token");
    assert_eq!(
        send(
            &app,
            "POST",
            &decision_path,
            &format!("{first}; {first_cookie}"),
            &decision.replace(&admin_csrf, &first_csrf)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "the original administrator cannot adjudicate their own hide"
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &decision_path,
            &format!("{moderator}; {moderator_cookie}"),
            &decision.replace(&admin_csrf, &moderator_csrf)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &decision_path, &second, &decision)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let admin_cookies = format!("{second}; {admin_cookie}");
    let (a, b) = tokio::join!(
        send(&app, "POST", &decision_path, &admin_cookies, &decision),
        send(&app, "POST", &decision_path, &admin_cookies, &decision)
    );
    let mut codes = [a.status().as_u16(), b.status().as_u16()];
    codes.sort();
    assert_eq!(
        codes,
        [
            StatusCode::SEE_OTHER.as_u16(),
            StatusCode::CONFLICT.as_u16()
        ]
    );
    assert_eq!(
        pg(
            &format!("SELECT visibility FROM discussion_comments WHERE id={comment}"),
            true
        ),
        "visible"
    );
    assert_eq!(
        pg(
            &format!("SELECT state FROM social_book_comment_appeals WHERE id={appeal}"),
            true
        ),
        "restored"
    );
    assert_eq!(
        pg(
            &format!(
                "SELECT count(*) FROM social_book_comment_appeal_decisions
             WHERE appeal_id={appeal} AND reviewer_user_id={second_id}
               AND decision='restored' AND note='Second review'"
            ),
            true
        ),
        "1"
    );
    pg(
        &format!(
        "UPDATE social_book_comment_appeal_decisions SET note='tampered' WHERE appeal_id={appeal}"
    ),
        false,
    );
    assert_eq!(
        pg(
            &format!(
                "SELECT visibility_status || ':' || admin_hold::text FROM books WHERE id={book}"
            ),
            true
        ),
        "active:false"
    );
    assert_eq!(
        pg(
            &format!("SELECT count(*) FROM library_purchased_chapters WHERE user_id={author_id}"),
            true
        ),
        "0"
    );
    let author_final = body(send(&app, "GET", author_path, &author, "").await).await;
    assert!(author_final.contains("Комментарий восстановлен"));
    // No test-only administrator may contaminate A's last-administrator checks.
    pg(
        &format!(
            "UPDATE account_roles SET is_active=false
         WHERE (user_id={first_id} OR user_id={second_id}) AND role='administrator'"
        ),
        true,
    );
}
