//! S049 Book complaints: real reader-to-administrator HTTP on isolated PG.
//! Never touches production and never gives a site moderator chapter powers.
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
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG only");
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

fn pg_rejected(query: &str) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let result = Command::new("psql")
        .args([
            "-X",
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
        !result.status.success(),
        "immutable audit must reject mutation"
    );
}

fn req(method: &str, path: &str, cookies: &str, form: &str, origin: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40491))));
    req
}

async fn send(app: &Router, method: &str, path: &str, cookies: &str, form: &str) -> Response {
    app.clone()
        .oneshot(req(method, path, cookies, form, ORIGIN))
        .await
        .unwrap()
}

async fn html(res: Response) -> String {
    String::from_utf8(
        to_bytes(res.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn cookie(res: &Response, name: &str) -> String {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(name))
        .expect("cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn field(page: &str, name: &str) -> String {
    page.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .expect("hidden form field")
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

async fn signup(app: &Router, email: &str, name: &str) -> String {
    let page = send(app, "GET", "/register", "", "").await;
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let csrf = field(&html(page).await, "csrf_token");
    let form = format!(
        "email={}&display_name={name}&password=LongSecurePassword2026&csrf_token={csrf}",
        email.replace('@', "%40")
    );
    let res = send(app, "POST", "/register", &csrf_cookie, &form).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    cookie(&res, "tor-dev-session=")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s049_book_report_http_live_age_visibility_csrf_admin_review_and_audit() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(catalog.pool()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email_reader = format!("s049-book-reader-{stamp}@example.invalid");
    let email_admin = format!("s049-book-admin-{stamp}@example.invalid");
    let email_mod = format!("s049-book-mod-{stamp}@example.invalid");
    let reader = signup(&app, &email_reader, "BookReporter").await;
    let admin = signup(&app, &email_admin, "BookAdmin").await;
    let moderator = signup(&app, &email_mod, "BookModerator").await;
    let reader_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='{email_reader}'"
    ))
    .parse()
    .unwrap();
    let admin_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='{email_admin}'"
    ))
    .parse()
    .unwrap();
    let moderator_id: i64 = pg(&format!(
        "SELECT id FROM users WHERE email_normalized='{email_mod}'"
    ))
    .parse()
    .unwrap();
    pg(&format!(
        "INSERT INTO account_roles(user_id,role)
         VALUES({admin_id},'administrator'),({moderator_id},'moderator')"
    ));
    let book: i64 = pg(&format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
         VALUES('ci:s049:book:{stamp}','ru','BookComplaintFixture','public','basic',{reader_id})
         RETURNING id"
    )).parse().unwrap();

    let report_path = format!("/books/{book}/report");
    assert_eq!(
        send(&app, "GET", &report_path, "", "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    let get = send(&app, "GET", &report_path, &reader, "").await;
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(get.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        get.headers()["x-robots-tag"],
        "noindex, nofollow, noarchive"
    );
    let csrf_cookie = cookie(&get, "tor-dev-csrf=");
    let form = html(get).await;
    assert!(form.contains("BookComplaintFixture"));
    let csrf = field(&form, "csrf_token");
    let key = field(&form, "command_key");
    let cookies = format!("{reader}; {csrf_cookie}");
    let post = format!(
        "reason=spam&details=%3Cscript%3Eunsafe()%3C%2Fscript%3E&command_key={key}&csrf_token={csrf}"
    );
    // Book visibility and age changes must invalidate a prepared form.
    pg(&format!(
        "UPDATE books SET visibility_status='hidden' WHERE id={book}"
    ));
    assert_eq!(
        send(&app, "GET", &report_path, &reader, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "POST", &report_path, &cookies, &post)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    pg(&format!(
        "UPDATE books SET visibility_status='active',access_level='adult' WHERE id={book}"
    ));
    assert_eq!(
        send(&app, "GET", &report_path, &reader, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "POST", &report_path, &cookies, &post)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    pg(&format!(
        "UPDATE books SET access_level='basic' WHERE id={book}"
    ));
    assert_eq!(
        send(&app, "POST", &report_path, &reader, &post)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone()
            .oneshot(req(
                "POST",
                &report_path,
                &cookies,
                &post,
                "https://evil.invalid"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &report_path, &cookies, &post)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    // Same command key is exactly-once; new key on same pending Book conflicts.
    assert_eq!(
        send(&app, "POST", &report_path, &cookies, &post)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let newer_key = "B".repeat(43);
    let newer = post.replace(&key, &newer_key);
    assert_eq!(
        send(&app, "POST", &report_path, &cookies, &newer)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let report: i64 = pg(&format!(
        "SELECT id FROM social_book_content_reports
         WHERE book_id={book} AND reporter_user_id={reader_id}"
    ))
    .parse()
    .unwrap();
    assert_eq!(
        pg(&format!(
            "SELECT COUNT(*) FROM social_book_content_reports WHERE book_id={book} AND state='pending'"
        )), "1"
    );

    let admin_path = "/admin/book-reports";
    assert_eq!(
        send(&app, "GET", admin_path, "", "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, "GET", admin_path, &reader, "").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "GET", admin_path, &moderator, "").await.status(),
        StatusCode::FORBIDDEN
    );
    let q = send(&app, "GET", admin_path, &admin, "").await;
    assert_eq!(q.status(), StatusCode::OK);
    assert_eq!(q.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(q.headers()["x-robots-tag"], "noindex, nofollow, noarchive");
    let q_cookie = cookie(&q, "tor-dev-csrf=");
    let q_html = html(q).await;
    assert!(q_html.contains(&format!("Жалоба #{report}")));
    assert!(q_html.contains("&lt;script&gt;unsafe()&lt;/script&gt;"));
    assert!(!q_html.contains("<script>unsafe()"));
    assert!(
        !q_html.contains("BookComplaintFixture"),
        "private queue contains IDs only"
    );
    let q_csrf = field(&q_html, "csrf_token");
    let admin_cookies = format!("{admin}; {q_cookie}");
    let decision_path = format!("/admin/book-reports/{report}/decision");
    let decision = format!("decision=reviewed&note=Confirmed&csrf_token={q_csrf}");
    assert_eq!(
        send(&app, "POST", &decision_path, &admin, &decision)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone()
            .oneshot(req(
                "POST",
                &decision_path,
                &admin_cookies,
                &decision,
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
            &decision_path,
            &admin_cookies,
            &format!("decision=block&note=No&csrf_token={q_csrf}")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    pg(&format!("UPDATE account_roles SET is_active=false WHERE user_id={admin_id} AND role='administrator'"));
    assert_eq!(
        send(&app, "POST", &decision_path, &admin_cookies, &decision)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    pg(&format!(
        "UPDATE account_roles SET is_active=true WHERE user_id={admin_id} AND role='administrator'"
    ));
    let (left, right) = tokio::join!(
        send(&app, "POST", &decision_path, &admin_cookies, &decision),
        send(&app, "POST", &decision_path, &admin_cookies, &decision)
    );
    let mut statuses = [left.status().as_u16(), right.status().as_u16()];
    statuses.sort();
    assert_eq!(
        statuses,
        [
            StatusCode::SEE_OTHER.as_u16(),
            StatusCode::CONFLICT.as_u16()
        ]
    );
    assert_eq!(
        pg(&format!(
            "SELECT state FROM social_book_content_reports WHERE id={report}"
        )),
        "reviewed"
    );
    assert_eq!(
        pg(&format!(
            "SELECT count(*) FROM social_book_content_review_events
             WHERE report_id={report} AND reviewer_user_id={admin_id}
               AND decision='reviewed' AND note='Confirmed'"
        )),
        "1"
    );
    pg_rejected(&format!(
        "UPDATE social_book_content_review_events SET note='tampered' WHERE report_id={report}"
    ));
    assert!(
        !html(send(&app, "GET", admin_path, &admin_cookies, "").await)
            .await
            .contains(&format!("Жалоба #{report}"))
    );
    assert_eq!(
        pg(&format!(
            "SELECT visibility_status || ':' || admin_hold::text FROM books WHERE id={book}"
        )),
        "active:false"
    );
    assert_eq!(
        pg(&format!(
            "SELECT count(*) FROM library_purchased_chapters WHERE user_id={reader_id}"
        )),
        "0"
    );
    // A visible public Book comment is a separate complaint target.
    let comment_key = "C".repeat(43);
    let comment: i64 = pg(&format!(
        "INSERT INTO discussion_comments
         (author_user_id,book_id,body,visibility,command_key,command_hash)
         VALUES({reader_id},{book},'Reported comment body','visible',
                '{comment_key}',decode(repeat('01',32),'hex')) RETURNING id"
    ))
    .parse()
    .unwrap();
    let comment_path = format!("{report_path}?comment_id={comment}");
    let comment_page = send(&app, "GET", &comment_path, &reader, "").await;
    assert_eq!(comment_page.status(), StatusCode::OK);
    let comment_csrf_cookie = cookie(&comment_page, "tor-dev-csrf=");
    let comment_form = html(comment_page).await;
    assert_eq!(field(&comment_form, "comment_id"), comment.to_string());
    let comment_csrf = field(&comment_form, "csrf_token");
    let comment_command = field(&comment_form, "command_key");
    let comment_cookies = format!("{reader}; {comment_csrf_cookie}");
    let comment_post = format!(
        "comment_id={comment}&reason=abuse&details=Review+comment&command_key={comment_command}&csrf_token={comment_csrf}"
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("{report_path}?comment_id={}", comment + 1000000),
            &reader,
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // A stale form for a hidden comment cannot register a complaint.
    pg(&format!(
        "UPDATE discussion_comments SET visibility='hidden' WHERE id={comment}"
    ));
    assert_eq!(
        send(&app, "POST", &report_path, &comment_cookies, &comment_post)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    pg(&format!(
        "UPDATE discussion_comments SET visibility='visible' WHERE id={comment}"
    ));
    assert_eq!(
        send(&app, "POST", &report_path, &comment_cookies, &comment_post)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "POST", &report_path, &comment_cookies, &comment_post)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let comment_report: i64 = pg(&format!(
        "SELECT id FROM social_book_content_reports WHERE book_id={book}
         AND comment_id={comment} AND reporter_user_id={reader_id}"
    ))
    .parse()
    .unwrap();
    assert_ne!(comment_report, report);
    let comments_queue = html(send(&app, "GET", admin_path, &admin_cookies, "").await).await;
    assert!(comments_queue.contains(&format!("Комментарий #{comment}")));
    assert!(!comments_queue.contains("Reported comment body"));
    let comment_decision = format!("/admin/book-reports/{comment_report}/decision");
    assert_eq!(
        send(&app, "POST", &comment_decision, &admin_cookies, &decision)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(&format!(
            "SELECT count(*) FROM social_book_content_review_events
             WHERE report_id={comment_report} AND decision='reviewed'"
        )),
        "1"
    );
    assert_eq!(
        pg(&format!(
            "SELECT visibility FROM discussion_comments WHERE id={comment}"
        )),
        "visible",
        "reviewing a report cannot silently moderate the comment"
    );
    // An explicit admin decision, unlike a reviewed report, may hide this
    // exact public Book comment. It cannot moderate an entire Book or chapter.
    let later_key = "D".repeat(43);
    let later_post = comment_post.replace(&comment_command, &later_key);
    assert_eq!(
        send(&app, "POST", &report_path, &comment_cookies, &later_post)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let next_report: i64 = pg(&format!(
        "SELECT id FROM social_book_content_reports WHERE book_id={book}
         AND comment_id={comment} AND reporter_user_id={reader_id} AND state='pending'"
    ))
    .parse()
    .unwrap();
    assert_ne!(next_report, comment_report);
    // A moderator may handle Book registration, never content complaints.
    assert_eq!(
        send(&app, "GET", admin_path, &moderator, "").await.status(),
        StatusCode::FORBIDDEN
    );
    let mod_cookies = format!("{moderator}; {q_cookie}");
    let queue_html = html(send(&app, "GET", admin_path, &admin_cookies, "").await).await;
    assert!(queue_html.contains("value=\"hide_comment\""));
    assert!(!queue_html.contains("Reported comment body"));
    let manual_hide = format!(
        "decision=hide_comment&note=Confirmed+%3Cscript%3Ealert%281%29%3C%2Fscript%3E&csrf_token={q_csrf}"
    );
    let manual_path = format!("/admin/book-reports/{next_report}/decision");
    // An active moderator still cannot hide a comment by forged POST.
    assert_eq!(
        send(&app, "POST", &manual_path, &mod_cookies, &manual_hide)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &manual_path, &admin_cookies, &manual_hide)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(&format!(
            "SELECT visibility FROM discussion_comments WHERE id={comment}"
        )),
        "hidden"
    );
    assert_eq!(
        pg(&format!(
            "SELECT decision || ':' || moderation_action
             FROM social_book_content_review_events WHERE report_id={next_report}"
        )),
        "reviewed:hide_book_comment"
    );
    assert_eq!(
        pg(&format!(
            "SELECT reviewer_user_id FROM social_book_content_review_events
             WHERE report_id={next_report}"
        )),
        admin_id.to_string()
    );
    pg_rejected(&format!(
        "UPDATE social_book_content_review_events SET moderation_action='none'
         WHERE report_id={next_report}"
    ));
    assert_eq!(
        send(&app, "POST", &manual_path, &admin_cookies, &manual_hide)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(&app, "GET", &comment_path, &reader, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(&format!(
            "SELECT visibility_status || ':' || admin_hold::text FROM books WHERE id={book}"
        )),
        "active:false",
        "manual comment hide must not block Book reading"
    );

    // The same action must not be available for a complaint about the Book.
    let book_key = "E".repeat(43);
    let later_book = post.replace(&key, &book_key);
    assert_eq!(
        send(&app, "POST", &report_path, &cookies, &later_book)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let book_only_report: i64 = pg(&format!(
        "SELECT id FROM social_book_content_reports
         WHERE book_id={book} AND comment_id IS NULL
           AND reporter_user_id={reader_id} AND state='pending'"
    ))
    .parse()
    .unwrap();
    let book_only_action = format!("/admin/book-reports/{book_only_report}/decision");
    assert_eq!(
        send(&app, "GET", admin_path, &mod_cookies, "").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &book_only_action,
            &mod_cookies,
            &format!("decision=reviewed&note=No+book+rights&csrf_token={q_csrf}")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &book_only_action,
            &mod_cookies,
            &format!("decision=dismissed&note=No+book+rights&csrf_token={q_csrf}")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &book_only_action, &mod_cookies, &manual_hide)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // The administrator's existing Book-only review semantics are unchanged.
    assert_eq!(
        send(
            &app,
            "POST",
            &book_only_action,
            &admin_cookies,
            &format!("decision=hide_comment&note=Invalid&csrf_token={q_csrf}")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        pg(&format!(
            "SELECT state FROM social_book_content_reports WHERE id={book_only_report}"
        )),
        "pending",
        "invalid sanction must leave the complaint unresolved"
    );
    // Immutable review history is public to neither readers nor guests.
    // Moderators cannot inspect complaint history of either target type.
    let history_response = send(&app, "GET", admin_path, &admin_cookies, "").await;
    assert_eq!(history_response.status(), StatusCode::OK);
    assert_eq!(
        history_response.headers()[header::CACHE_CONTROL],
        "no-store"
    );
    let history_html = html(history_response).await;
    assert!(history_html.contains("История решений"));
    assert!(history_html.contains(&format!("Решение по жалобе #{report}")));
    assert!(history_html.contains(&format!("Решение по жалобе #{next_report}")));
    assert!(history_html.contains("hide_book_comment"));
    assert!(history_html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!history_html.contains("<script>alert(1)</script>"));
    assert!(!history_html.contains("BookComplaintFixture"));
    assert!(!history_html.contains("Reported comment body"));
    assert_eq!(
        send(&app, "GET", admin_path, &mod_cookies, "").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "GET", admin_path, &reader, "").await.status(),
        StatusCode::FORBIDDEN
    );
    let event_id: i64 = pg(&format!(
        "SELECT id FROM social_book_content_review_events WHERE report_id={next_report}"
    ))
    .parse()
    .unwrap();
    let older_path = format!("{admin_path}?history_before={event_id}");
    let older = html(send(&app, "GET", &older_path, &admin_cookies, "").await).await;
    assert!(!older.contains(&format!("Решение по жалобе #{next_report}")));
    assert!(older.contains(&format!("Решение по жалобе #{report}")));
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("{admin_path}?history_before=0"),
            &admin_cookies,
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    // Prevent a test-only active administrator from contaminating the later
    // A-04/A-05 last-admin invariant on the same disposable PostgreSQL.
    pg(&format!(
        "UPDATE account_roles SET is_active=false
         WHERE user_id={admin_id} AND role='administrator'"
    ));
    assert_eq!(
        send(&app, "GET", admin_path, &admin_cookies, "")
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "revoked site administrator must not read review history"
    );
    pg(&format!(
        "UPDATE account_roles SET is_active=false
         WHERE user_id={moderator_id} AND role='moderator'"
    ));
    assert_eq!(
        send(&app, "GET", admin_path, &mod_cookies, "")
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "revoked site moderator must not read review history"
    );
}
