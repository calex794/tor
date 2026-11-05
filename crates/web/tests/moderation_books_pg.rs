//! A-05: real PostgreSQL HTTP moderation and fail-closed authorization.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::{
    repositories::{postgres::PgAuthoring, PgCatalog},
    services::authoring::{new_book, save_chapter, SaveChapterRequest},
};
use tor_contracts::auth::{AccountRole, UserId};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let output = std::process::Command::new("psql")
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
        .expect("psql installed");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    body: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn moderator_can_hold_and_release_book_without_publication_or_admin_privileges() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("ModerationBookHoldTestPassword2026!").unwrap();
    let ttl = NonZeroU32::new(3600).unwrap();
    let admin_token = generate_session_token();
    let moderator_token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let admin = accounts
        .register_with_session(
            &format!("mod-books-admin-{nonce}@example.invalid"),
            "Admin",
            &password,
            &session_token_digest(&admin_token),
            ttl,
        )
        .await
        .unwrap();
    let moderator = accounts
        .register_with_session(
            &format!("mod-books-moderator-{nonce}@example.invalid"),
            "Moderator",
            &password,
            &session_token_digest(&moderator_token),
            ttl,
        )
        .await
        .unwrap();
    let book_owner = accounts
        .register_with_session(
            &format!("mod-books-outsider-{nonce}@example.invalid"),
            "Outsider",
            &password,
            &session_token_digest(&outsider_token),
            ttl,
        )
        .await
        .unwrap();
    // The fixture alone bootstraps one administrator, never a user endpoint.
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \
             VALUES({admin},'administrator',TRUE)"
        ),
    );
    accounts
        .change_account_role(
            &session_token_digest(&admin_token),
            UserId::new(moderator).unwrap(),
            AccountRole::Moderator,
            true,
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES('ci:moderation:book:{nonce}','ru',
             '<script>private moderation title</script>','public','basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let path = "/moderation/books";
    let action = format!("/moderation/books/{book}/hold");
    let mod_cookie = format!("tor-dev-session={moderator_token}; tor-dev-csrf={csrf}");
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    let outsider_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");

    // Book review is the only moderation entrypoint. No chapter queue exists.
    for (method, path) in [
        ("GET", "/moderation/chapters".to_owned()),
        ("POST", format!("/moderation/chapters/{book}/hold")),
    ] {
        assert_eq!(
            request(&app, method, &path, &admin_cookie, ORIGIN, "")
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "chapter-by-chapter moderation is not a product capability"
        );
    }

    assert_eq!(
        request(&app, "GET", path, "", ORIGIN, "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", path, &outsider_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &action,
            &outsider_cookie,
            ORIGIN,
            &format!("csrf_token={csrf}&expected_hold=false&new_hold=true")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let page = request(&app, "GET", path, &mod_cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let content = html(page).await;
    assert!(content.contains("&lt;script&gt;private moderation title&lt;/script&gt;"));
    assert!(!content.contains("<script>private moderation title</script>"));
    assert!(!content.contains("body_markdown"));

    // S020: unlike Book team membership, site moderation controls the
    // four-state visibility independent of author draft/public and hold.
    let visibility_action = format!("/moderation/books/{book}/visibility");
    assert!(content.contains(&visibility_action));
    let before_total: i64 = pg(
        &url,
        "SELECT visible_book_count FROM public_catalog_statistics WHERE singleton",
    )
    .parse()
    .unwrap();
    let hidden_form =
        format!("csrf_token={csrf}&expected_status=active&expected_revision=1&new_status=hidden");
    assert_eq!(
        request(
            &app,
            "POST",
            &visibility_action,
            &outsider_cookie,
            ORIGIN,
            &hidden_form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &visibility_action,
            &mod_cookie,
            "https://evil.invalid",
            &hidden_form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &visibility_action,
            &mod_cookie,
            ORIGIN,
            &format!("csrf_token={csrf}&expected_status=active&expected_revision=1&new_status=bad")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &visibility_action,
            &mod_cookie,
            ORIGIN,
            &hidden_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT visibility||':'||visibility_status||':'||admin_hold::int
             FROM books WHERE id={book}"
            )
        ),
        "public:hidden:0"
    );
    assert!(PgReader::new(pool.clone())
        .public_book_page(book as u64, 10, None)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        request(&app, "GET", &format!("/books/{book}"), "", ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let hidden_total: i64 = pg(
        &url,
        "SELECT visible_book_count FROM public_catalog_statistics WHERE singleton",
    )
    .parse()
    .unwrap();
    assert_eq!(hidden_total, before_total - 1);
    assert_eq!(
        request(
            &app,
            "POST",
            &visibility_action,
            &mod_cookie,
            ORIGIN,
            &hidden_form
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let active_form =
        format!("csrf_token={csrf}&expected_status=hidden&expected_revision=2&new_status=active");
    assert_eq!(
        request(
            &app,
            "POST",
            &visibility_action,
            &mod_cookie,
            ORIGIN,
            &active_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(PgReader::new(pool.clone())
        .public_book_page(book as u64, 10, None)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility_status_revision FROM books WHERE id={book}")
        ),
        "3"
    );
    let after_total: i64 = pg(
        &url,
        "SELECT visible_book_count FROM public_catalog_statistics WHERE singleton",
    )
    .parse()
    .unwrap();
    assert_eq!(after_total, before_total);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT actor_user_id || ':' || before_status || '>' || after_status
             FROM moderation_book_visibility_events WHERE book_id={book} ORDER BY id"
            )
        ),
        format!("{moderator}:active>hidden\n{moderator}:hidden>active")
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/moderation/books?before=-1",
            &mod_cookie,
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let form = format!("csrf_token={csrf}&expected_hold=false&new_hold=true");
    assert_eq!(
        request(
            &app,
            "POST",
            &action,
            &mod_cookie,
            "https://evil.invalid",
            &form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &action,
            &format!("tor-dev-session={moderator_token}"),
            ORIGIN,
            &form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let applied = request(&app, "POST", &action, &mod_cookie, ORIGIN, &form).await;
    assert_eq!(applied.status(), StatusCode::SEE_OTHER);
    let current = pg(
        &url,
        &format!("SELECT visibility || ':' || admin_hold::int FROM books WHERE id={book}"),
    );
    assert_eq!(current, "public:1");
    assert_eq!(
        request(&app, "GET", &format!("/books/{book}"), "", ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "POST", &action, &mod_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let audit_count: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM moderation_book_hold_events WHERE book_id={book}"),
    )
    .parse()
    .unwrap();
    assert_eq!(audit_count, 1, "stale submit must not duplicate audit");

    accounts
        .change_account_role(
            &session_token_digest(&admin_token),
            UserId::new(moderator).unwrap(),
            AccountRole::Moderator,
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        request(&app, "GET", path, &mod_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let blocked_form =
        format!("csrf_token={csrf}&expected_status=active&expected_revision=3&new_status=blocked");
    assert_eq!(
        request(
            &app,
            "POST",
            &visibility_action,
            &mod_cookie,
            ORIGIN,
            &blocked_form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "revoked site role has no moderation visibility rights"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &visibility_action,
            &admin_cookie,
            ORIGIN,
            &blocked_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(PgReader::new(pool.clone())
        .public_book_page(book as u64, 10, None)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT visibility||':'||visibility_status||':'||admin_hold::int
             FROM books WHERE id={book}"
            )
        ),
        "public:blocked:1"
    );
    // PostgreSQL trigger must reject deleting audit evidence. Keep the
    // check within tor-web's existing psql-backed integration harness.
    pg(
        &url,
        &format!(
            "DO $$
             BEGIN
                 BEGIN
                     DELETE FROM moderation_book_visibility_events WHERE book_id={book};
                     RAISE EXCEPTION 'visibility audit was mutable';
                 EXCEPTION WHEN check_violation THEN
                     IF NOT EXISTS (
                         SELECT 1 FROM moderation_book_visibility_events WHERE book_id={book}
                     ) THEN
                         RAISE EXCEPTION 'visibility audit unexpectedly absent';
                     END IF;
                 END;
             END $$;"
        ),
    );
    let release = format!("csrf_token={csrf}&expected_hold=true&new_hold=false");
    assert_eq!(
        request(&app, "POST", &action, &mod_cookie, ORIGIN, &release)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", &action, &admin_cookie, ORIGIN, &release)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let current = pg(
        &url,
        &format!("SELECT visibility || ':' || admin_hold::int FROM books WHERE id={book}"),
    );
    assert_eq!(current, "public:0");
    let events = pg(
        &url,
        &format!(
            "SELECT actor_user_id || ':' || before_hold::int || ':' || after_hold::int
             FROM moderation_book_hold_events WHERE book_id={book} ORDER BY id"
        ),
    );
    assert_eq!(events, format!("{moderator}:0:1\n{admin}:1:0"));
    // A newly registered book is pending site review even after the author
    // publishes its first free chapter. The Book owner cannot approve it.
    let author = PgAuthoring::new(pool.clone());
    let registration = new_book(
        &format!("ci:moderation:fresh:{nonce}"),
        "ru",
        "Новая книга на проверке",
        &"t".repeat(43),
    )
    .unwrap();
    let fresh = author
        .create_owned_book(book_owner, &registration)
        .await
        .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility||':'||visibility_status FROM books WHERE id={fresh}")
        ),
        "draft:pending_moderation"
    );
    let first = save_chapter(SaveChapterRequest {
        book_id: fresh as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Глава перед модерацией",
        body: "Текст ожидает одобрения",
        access: "free",
        price_amount: 0,
        command_key: &"u".repeat(43),
    })
    .unwrap();
    author
        .create_free_chapter(book_owner, &first)
        .await
        .unwrap();
    author.release_book(book_owner, fresh).await.unwrap();
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility||':'||visibility_status FROM books WHERE id={fresh}")
        ),
        "public:pending_moderation"
    );
    assert!(PgReader::new(pool.clone())
        .public_book_page(fresh as u64, 10, None)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        request(&app, "GET", &format!("/books/{fresh}"), "", ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let approve_action = format!("/moderation/books/{fresh}/visibility");
    let approve = format!(
        "csrf_token={csrf}&expected_status=pending_moderation&expected_revision=1&new_status=active"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &approve_action,
            &outsider_cookie,
            ORIGIN,
            &approve
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", &approve_action, &mod_cookie, ORIGIN, &approve)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "revoked site moderator cannot approve a new book"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &approve_action,
            &admin_cookie,
            ORIGIN,
            &approve
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "generic visibility editor cannot approve an unreviewed Book registration"
    );
    // Only the separate site registration approval action can publish the
    // moderation decision. Chapter release remains outside this review.
    let registration_action = format!("/moderation/books/{fresh}/registration");
    let registration_approve = format!("csrf_token={csrf}&expected_revision=1&action=approve");
    assert_eq!(
        request(
            &app,
            "POST",
            &registration_action,
            &admin_cookie,
            ORIGIN,
            &registration_approve
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &approve_action,
            &admin_cookie,
            ORIGIN,
            &approve
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "repeated generic approval must fail CAS"
    );
    assert!(PgReader::new(pool.clone())
        .public_book_page(fresh as u64, 10, None)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT actor_user_id || ':' || before_status || '>' || after_status
                 FROM moderation_book_visibility_events
                 WHERE book_id={fresh}"
            )
        ),
        format!("{admin}:pending_moderation>active")
    );

    // System history combines decisions but is invisible to Book owners and
    // logged-out readers, including reviewer name and refusal reasons.
    let history_path = format!("/moderation/books/{fresh}/history");
    assert_eq!(
        request(&app, "GET", &history_path, "", ORIGIN, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", &history_path, &outsider_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let history_response = request(&app, "GET", &history_path, &admin_cookie, ORIGIN, "").await;
    assert_eq!(history_response.status(), StatusCode::OK);
    assert_eq!(
        history_response.headers()[header::CACHE_CONTROL],
        "no-store"
    );
    let history_body = html(history_response).await;
    assert!(history_body.contains("Регистрация: Одобрено"));
    assert!(history_body.contains("Модератор: Admin"));
    assert!(history_body.contains(&format!("(#{admin})")));
    assert!(history_body.contains("Все решения") || history_body.contains("Системная история"));
    assert!(!history_body.contains("<script>private moderation title</script>"));

    // After book approval, an author releases the next chapter directly.
    let second = save_chapter(SaveChapterRequest {
        book_id: fresh as u64,
        chapter_id: None,
        number: 2,
        expected_revision: 0,
        title: "Глава после одобрения книги",
        body: "Новая глава публикуется без дополнительной модерации",
        access: "free",
        price_amount: 0,
        command_key: &"v".repeat(43),
    })
    .unwrap();
    let saved = author
        .create_pending_free_chapter(book_owner, &second)
        .await
        .unwrap();
    author
        .release_pending_free_chapter(
            book_owner,
            fresh,
            saved.chapter_id,
            saved.revision,
            saved.version_id,
        )
        .await
        .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT visibility || ':' || admin_hold::int || ':' || moderation_state
                 FROM chapters WHERE id={}",
                saved.chapter_id
            )
        ),
        "public:0:none"
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/reader/{}", saved.chapter_id),
            "",
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::OK
    );

    // Only a test fixture may toggle the pre-existing legacy hold. The
    // chapter moderation routes were removed; this checks fail-closed reads
    // without restoring any chapter-by-chapter approval workflow.
    pg(
        &url,
        &format!(
            "UPDATE chapters SET admin_hold=TRUE WHERE id={}",
            saved.chapter_id
        ),
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT visibility || ':' || admin_hold::int || ':' || moderation_state
                 FROM chapters WHERE id={}",
                saved.chapter_id
            )
        ),
        "public:1:hold"
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/reader/{}", saved.chapter_id),
            "",
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "legacy-held chapters must not become readable just because review routes are gone"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility || ':' || visibility_status FROM books WHERE id={fresh}")
        ),
        "public:active",
        "legacy chapter hold must not silently change book moderation"
    );
}

#[path = "moderation_books_pg/registration_cases.rs"]
mod registration_cases;
