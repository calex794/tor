//! C-08/BR-060: reader opens exact private saved-free V1, not current V2.
//! Only disposable PostgreSQL tor_ci. No normal-delete production switch.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
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
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production DB forbidden");
    let result = Command::new("psql")
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
        .expect("psql client");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}

async fn call(app: &Router, method: &str, url: &str, cookie: &str, body: &str) -> Response {
    let mut req = Request::builder()
        .method(method)
        .uri(url)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            41001,
        ))));
    app.clone().oneshot(req).await.unwrap()
}

async fn json_call(
    app: &Router,
    method: &str,
    url: &str,
    cookie: &str,
    csrf: &str,
    body: &str,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(url)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-csrf-token", csrf)
        .body(Body::from(body.to_owned()))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            41002,
        ))));
    app.clone().oneshot(request).await.unwrap()
}

fn auth_cookie(response: &Response, prefix: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(prefix))
        .map(|value| value.split(';').next().unwrap_or_default().to_owned())
        .expect("auth Set-Cookie")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn c_08_saved_free_v1_is_private_after_v2_and_pending_deletion() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "no permanent database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let marker = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let password = hash_password("C08SavedReader2026").unwrap();
    let actor = accounts
        .register_with_session(
            &format!("c08-reader-{marker}@example.invalid"),
            "C08 reader",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let outsider = accounts
        .register_with_session(
            &format!("c08-outsider-{marker}@example.invalid"),
            "C08 outsider",
            &password,
            &session_token_digest(&outsider_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
             VALUES('ci:c08:reader:{marker}','ru','Archived <unsafe>','public','basic',{actor})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
             VALUES({book},1,'Title <unsafe>','public','free') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let v1: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Pinned ORIGINAL V1') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{v1})"
        ),
    );
    // A directly purchased chapter can belong to a Book never bookmarked by
    // this reader. It must still be a private Book group after ordinary deletion.
    let paid_only_book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
             VALUES('ci:br060:paid-only:{marker}','ru','Paid-only <unsafe>','public','basic',{outsider})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let paid_chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({paid_only_book},1,'Bought chapter','public','paid',12900) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let paid_v1: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({paid_chapter},1,'Purchased immutable V1') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({paid_chapter},{paid_v1})"
        ),
    );
    let paid_purchase: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_purchase_facts
             (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
             VALUES({actor},{paid_chapter},{paid_v1},'br060-paid-only-{marker}',12900,'RUB')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();

    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let reader_cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let outsider_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");
    let read = format!("/me/library/saved/{chapter}");
    let list = format!("/me/library/{book}/saved");

    assert_eq!(
        call(&app, "GET", &read, &reader_cookie, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/me/library/{book}"),
            &reader_cookie,
            &format!("csrf_token={csrf}")
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let saved = call(&app, "GET", &read, &reader_cookie, "").await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(saved.headers()[header::CACHE_CONTROL], "no-store");
    let before_csp = saved.headers()["content-security-policy"]
        .to_str()
        .expect("pre-deletion CSP");
    assert!(before_csp.contains("script-src 'none'"));
    assert!(!before_csp.contains("connect-src 'self'"));
    let text = html(saved).await;
    assert!(text.contains("Pinned ORIGINAL V1"));
    assert!(text.contains("Archived &lt;unsafe&gt;"));
    assert!(!text.contains("Archived <unsafe>"));
    let contents = html(call(&app, "GET", &list, &reader_cookie, "").await).await;
    assert!(contents.contains(&format!("href=\"{read}\"")));
    assert!(
        !contents.contains("Pinned ORIGINAL V1"),
        "TOC is metadata only"
    );

    // Private saved-free reader cannot be accessed by another signed-in reader.
    assert_eq!(
        call(&app, "GET", &read, &outsider_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_ne!(
        call(&app, "GET", &read, "", "").await.status(),
        StatusCode::OK
    );

    // The position is recorded on genuinely published V1, prior to author
    // publishing V2 and permanently withdrawing this Book.
    pg(
        &url,
        &format!(
            "INSERT INTO reader_book_positions
             (user_id,book_id,chapter_id,chapter_no,chapter_revision,
              version_id,block_index,char_offset)
             VALUES({actor},{book},{chapter},1,1,{v1},1,6)"
        ),
    );

    let v2: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},2,'Current V2 should stay outside private V1 pin')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!("UPDATE chapter_publications SET version_id={v2} WHERE chapter_id={chapter}"),
    );
    let deletion = format!("/me/books/{book}/settings/delete");
    assert_eq!(
        call(&app, "GET", &deletion, &outsider_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let first_form = html(call(&app, "GET", &deletion, &reader_cookie, "").await).await;
    assert!(first_form.contains("value=\"start\""));
    assert_eq!(
        call(
            &app,
            "POST",
            &deletion,
            &reader_cookie,
            &format!("csrf_token={csrf}&action=start&expected_revision=1&account_password=wrong")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &deletion,
            &reader_cookie,
            &format!("csrf_token={csrf}&action=start&expected_revision=1&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let pending_form = html(call(&app, "GET", &deletion, &reader_cookie, "").await).await;
    assert!(!pending_form.contains("value=\"complete\""));
    assert!(pending_form.contains("Заявка направлена на модерацию"));
    assert_eq!(
        call(
            &app, "POST", &deletion, &reader_cookie,
            &format!("csrf_token={csrf}&action=complete&expected_revision=2&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
        "owners cannot finalize deletion even after approval"
    );
    // Even a site moderator who owns the Book cannot self-approve deletion.
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({actor},'moderator',TRUE)"
        ),
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/moderation/books/{book}/deletion"),
            &reader_cookie,
            &format!("csrf_token={csrf}&request_revision=2&decision=approve")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let moderator_queue = "/moderation/books/deletions";
    assert_eq!(
        call(&app, "GET", moderator_queue, &outsider_cookie, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({outsider},'moderator',TRUE)"
        ),
    );
    let queue_html = html(call(&app, "GET", moderator_queue, &outsider_cookie, "").await).await;
    assert!(queue_html.contains("Archived &lt;unsafe&gt;"));
    assert!(queue_html.contains(&format!("/moderation/books/{book}/deletion")));
    let review_action = format!("/moderation/books/{book}/deletion");
    // Waiting time is measured from the original owner deletion request,
    // not the content's 18+/21+ rating or the Book's creation date.
    let recent_queue = html(
        call(&app, "GET", "/moderation/books/deletions?wait=lt24", &outsider_cookie, "")
            .await,
    )
    .await;
    assert!(recent_queue.contains(&review_action));
    let old_queue = html(
        call(&app, "GET", "/moderation/books/deletions?wait=gte72", &outsider_cookie, "")
            .await,
    )
    .await;
    assert!(!old_queue.contains(&review_action));
    assert_eq!(
        call(&app, "GET", "/moderation/books/deletions?wait=unknown",
            &outsider_cookie, "").await.status(),
        StatusCode::BAD_REQUEST
    );
    // Audit timestamps remain immutable. A fresh request must not match the
    // three-day waiting bucket; the filter does not modify its source row.
    assert_eq!(
        call(
            &app,
            "POST",
            &review_action,
            &reader_cookie,
            &format!("csrf_token={csrf}&request_revision=2&decision=approve")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &review_action,
            &outsider_cookie,
            "csrf_token=wrong&request_revision=2&decision=approve"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &review_action,
            &outsider_cookie,
            &format!("csrf_token={csrf}&request_revision=2&decision=approve")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &review_action,
            &outsider_cookie,
            &format!("csrf_token={csrf}&request_revision=2&decision=approve")
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "a decision is immutable and cannot be applied twice"
    );
    let approved_form = html(call(&app, "GET", &deletion, &reader_cookie, "").await).await;
    assert!(!approved_form.contains("value=\"complete\""));
    assert!(approved_form.contains("удалена"));
    let pending_library = html(call(&app, "GET", "/me/library", &reader_cookie, "").await).await;
    assert!(pending_library.contains("Удалена"));
    let pending = html(call(&app, "GET", &read, &reader_cookie, "").await).await;
    assert!(pending
        .replace("<span id=\"saved-resume\"></span>", "")
        .contains("Pinned ORIGINAL V1"));
    assert!(!pending.contains("Current V2 should stay outside private V1 pin"));

    // A completed ordinary deletion deindexes public pages but keeps the
    // saved exact private edition and Book tombstone for its original reader.
    assert_eq!(
        call(
            &app, "POST", &deletion, &outsider_cookie,
            &format!("csrf_token={csrf}&action=complete&expected_revision=2&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &deletion,
            &reader_cookie,
            &format!("csrf_token={csrf}&action=complete&expected_revision=2&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let events = pg(
        &url,
        &format!("SELECT COUNT(*) FROM book_ordinary_deletion_events WHERE book_id={book}"),
    );
    assert_eq!(events, "2");

    // A separate ordinary deletion request can be rejected. Rejection
    // restores both original lifecycle and original public visibility,
    // and cannot be replayed as an approval for a later request.
    let rejected_book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                               visibility,access_level)
             VALUES('ci:c08:reject:{marker}','ru','Rejected deletion',{actor},
                    'public','basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let rejected_path = format!("/me/books/{rejected_book}/settings/delete");
    assert_eq!(
        call(
            &app, "POST", &rejected_path, &reader_cookie,
            &format!("csrf_token={csrf}&action=start&expected_revision=1&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let reject_action = format!("/moderation/books/{rejected_book}/deletion");
    let missing_reason = format!("csrf_token={csrf}&request_revision=2&decision=reject");
    assert_eq!(
        call(
            &app,
            "POST",
            &reject_action,
            &outsider_cookie,
            &missing_reason
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        call(
            &app,
            "POST",
            &reject_action,
            &outsider_cookie,
            &format!("csrf_token={csrf}&request_revision=2&decision=reject&reason=denied")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT lifecycle_status || ':' || visibility || ':' || lifecycle_revision::text
                 FROM books WHERE id={rejected_book}"
            )
        ),
        "in_progress:public:3"
    );
    assert_eq!(
        call(
            &app, "POST", &rejected_path, &reader_cookie,
            &format!("csrf_token={csrf}&action=complete&expected_revision=3&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &reject_action,
            &outsider_cookie,
            &format!("csrf_token={csrf}&request_revision=2&decision=approve")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let second_start = html(call(&app, "GET", &rejected_path, &reader_cookie, "").await).await;
    assert!(second_start.contains("value=\"start\""));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT decision FROM book_ordinary_deletion_decisions
                 WHERE book_id={rejected_book} AND request_revision=2"
            )
        ),
        "reject"
    );
    pg(
        &url,
        &format!(
            "UPDATE books SET lifecycle_status='ordinary_deleted',
             ordinary_deleted_at=now(),visibility='draft' WHERE id={paid_only_book}"
        ),
    );
    let paid_only_card = html(call(&app, "GET", "/me/library", &reader_cookie, "").await).await;
    assert!(paid_only_card.contains("Paid-only &lt;unsafe&gt;"));
    assert!(paid_only_card.contains("Книги с покупками"));
    assert!(paid_only_card.contains("чтение временно недоступно"));
    assert!(!paid_only_card.contains(&format!("action=\"/me/library/{paid_only_book}/remove\"")));

    let deleted_form = html(call(&app, "GET", &deletion, &reader_cookie, "").await).await;
    assert!(!deleted_form.contains("type=\"password\""));
    assert_eq!(
        call(&app, "GET", &format!("/books/{book}"), "", "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &format!("/read/{chapter}"), &reader_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let tombstone = html(call(&app, "GET", "/me/library", &reader_cookie, "").await).await;
    assert!(tombstone.contains("Удалена"));
    assert!(tombstone.contains(&format!("/me/library/{book}/saved")));
    let archived_response = call(&app, "GET", &read, &reader_cookie, "").await;
    let archive_csp = archived_response.headers()["content-security-policy"]
        .to_str()
        .expect("archived CSP");
    assert!(archive_csp.contains("script-src 'self'"));
    assert!(
        archive_csp.contains("connect-src 'self'"),
        "archived Chromium autosave must be allowed to connect to same-origin API"
    );
    let still_saved = html(archived_response).await;
    assert!(still_saved
        .replace("<span id=\"saved-resume\"></span>", "")
        .contains("Pinned ORIGINAL V1"));
    assert!(!still_saved.contains("Current V2 should stay outside private V1 pin"));
    // A second, real HTTP login after final ordinary deletion must recover
    // exactly the pre-deletion V1 and its saved UTF-16 reading position.
    let login_page = call(&app, "GET", "/login", "", "").await;
    assert_eq!(login_page.status(), StatusCode::OK);
    let login_cookie = auth_cookie(&login_page, "tor-dev-csrf=");
    let login_html = html(login_page).await;
    let login_csrf = login_html
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .expect("login CSRF input")
        .split('"')
        .next()
        .expect("login CSRF token");
    let logged = call(
        &app,
        "POST",
        "/login",
        &login_cookie,
        &format!(
            "email=c08-reader-{marker}%40example.invalid&password=C08SavedReader2026&csrf_token={login_csrf}"
        ),
    )
    .await;
    assert_eq!(logged.status(), StatusCode::SEE_OTHER);
    let fresh_session = auth_cookie(&logged, "tor-dev-session=");
    assert_ne!(fresh_session, format!("tor-dev-session={token}"));
    let new_reader_cookie = format!("{fresh_session}; {login_cookie}");
    let fresh_library = html(call(&app, "GET", "/me/library", &new_reader_cookie, "").await).await;
    assert!(fresh_library.contains("Удалена"));
    assert!(fresh_library.contains(&format!("/me/library/{book}/saved")));
    assert!(fresh_library.contains("Paid-only &lt;unsafe&gt;"));
    assert!(fresh_library.contains("Книги с покупками"));
    assert!(!fresh_library.contains(&format!("action=\"/me/library/{paid_only_book}/remove\"")));
    let paid_list_path = format!("/me/library/{paid_only_book}/saved");
    let paid_list_response = call(&app, "GET", &paid_list_path, &new_reader_cookie, "").await;
    assert_eq!(paid_list_response.status(), StatusCode::OK);
    let paid_list = html(paid_list_response).await;
    assert!(paid_list.contains("Купленная редакция 1"));
    assert!(paid_list.contains("чтение временно недоступно"));
    assert!(!paid_list.contains(&format!("/read/paid/{paid_chapter}")));
    let outsider_library = html(call(&app, "GET", "/me/library", &outsider_cookie, "").await).await;
    assert!(!outsider_library.contains("Paid-only &lt;unsafe&gt;"));

    let fresh_read = html(call(&app, "GET", &read, &new_reader_cookie, "").await).await;
    assert!(fresh_read
        .replace("<span id=\"saved-resume\"></span>", "")
        .contains("Pinned ORIGINAL V1"));
    assert!(fresh_read.contains("href=\"#saved-resume\""));
    assert!(fresh_read.contains("data-free-progress-archive=\"1\""));
    assert!(fresh_read.contains("data-free-progress-version=\""));
    assert!(fresh_read.contains("/assets/newboo-archive-progress.js"));
    assert!(!fresh_read.contains("Current V2 should stay outside private V1 pin"));

    // After a real second login, the user can advance the old V1 position.
    // The API is independently scoped to the exact pin, not public V2.
    let archived_url = "/api/v1/me/progress/archived-free";
    let state_url = format!("{archived_url}?chapter_id={chapter}&version_id={v1}");
    let first_state = call(&app, "GET", &state_url, &new_reader_cookie, "").await;
    assert_eq!(first_state.status(), StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&html(first_state).await).unwrap();
    let revision = value["revision"].as_i64().unwrap();
    assert_eq!(value["version_id"].as_str(), Some(v1.to_string().as_str()));
    assert_eq!(value["char_offset"].as_i64(), Some(6));
    let saved_command = serde_json::json!({
        "chapter_id": chapter.to_string(),
        "version_id": v1.to_string(),
        "block_id": 1,
        "char_offset": 10,
        "expected_revision": revision,
    })
    .to_string();
    let advanced = json_call(
        &app,
        "PUT",
        archived_url,
        &new_reader_cookie,
        login_csrf,
        &saved_command,
    )
    .await;
    assert_eq!(advanced.status(), StatusCode::OK);
    let receipt: serde_json::Value = serde_json::from_str(&html(advanced).await).unwrap();
    assert!(receipt["revision"].as_i64().unwrap() > revision);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT char_offset FROM reader_book_positions WHERE user_id={actor} AND book_id={book}"
            )
        ),
        "10"
    );
    assert_eq!(
        json_call(
            &app,
            "PUT",
            archived_url,
            &new_reader_cookie,
            login_csrf,
            &saved_command,
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "a replay with stale CAS revision must be rejected"
    );
    assert_eq!(
        json_call(
            &app,
            "PUT",
            archived_url,
            &new_reader_cookie,
            "wrong-token",
            &saved_command,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_call(
            &app,
            "PUT",
            archived_url,
            &outsider_cookie,
            &csrf,
            &saved_command,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{archived_url}?chapter_id={chapter}&version_id={v2}"),
            &new_reader_cookie,
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT COUNT(*) FROM reader_chapter_completions WHERE user_id={actor} AND chapter_id={chapter}"
            )
        ),
        "0",
        "saved scrolling is never an assertion that a chapter was finished"
    );
    let moved = html(call(&app, "GET", &read, &new_reader_cookie, "").await).await;
    assert!(moved.contains("href=\"#saved-resume\""));

    assert_eq!(
        call(&app, "GET", &read, &outsider_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={book}"),
    );
    assert_eq!(
        call(&app, "GET", &read, &reader_cookie, "").await.status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=FALSE, access_level='adult' WHERE id={book}"),
    );
    assert_eq!(
        call(&app, "GET", &read, &reader_cookie, "").await.status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!("UPDATE books SET access_level='basic' WHERE id={book}"),
    );
    // The old session cannot read the archive after logout/revocation;
    // the second login still can, until the reader removes the saved Book.
    assert!(PgAccounts::from_pool(pool.clone())
        .revoke_session(&session_token_digest(&token))
        .await
        .unwrap());
    assert_ne!(
        call(&app, "GET", &read, &reader_cookie, "").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", &read, &new_reader_cookie, "")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/me/library/{book}/remove"),
            &new_reader_cookie,
            &format!("csrf_token={login_csrf}")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "GET", &read, &new_reader_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // Removing the direct purchase must hide the Book group, without touching
    // the separate free-book archive that this same actor previously removed.
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_purchase_revocations
             (purchase_id,reversal_reference,reason)
             VALUES({paid_purchase},'br060-paid-only-refund-{marker}','refund')"
        ),
    );
    let after_refund = html(call(&app, "GET", "/me/library", &new_reader_cookie, "").await).await;
    assert!(!after_refund.contains("Paid-only &lt;unsafe&gt;"));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT COUNT(*) FROM chapter_purchase_facts WHERE id={paid_purchase}")
        ),
        "1",
        "refund must not delete financial history"
    );

    // Ordinary deletion retains the Book as an audited tombstone.
    // The disposable tor_ci database is discarded after this job, so fixture
    // cleanup must not physically delete records protected by RESTRICT.
    let deletion_sql = format!("DELETE FROM books WHERE id={book}");
    let refused = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url.as_str(),
            "--command",
            deletion_sql.as_str(),
        ])
        .output()
        .expect("psql client");
    assert!(
        !refused.status.success()
            && String::from_utf8_lossy(&refused.stderr).contains("book_members_book_id_fkey"),
        "ordinary deletion must not physically erase a Book with protected membership"
    );
    assert_eq!(
        pg(&url, &format!("SELECT COUNT(*) FROM books WHERE id={book}")),
        "1"
    );
    pool.close().await;
}
