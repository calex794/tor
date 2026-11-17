//! RET-01/R01: a historical file import is not a new subscribed release.
//! Real author import, moderation, Follow HTTP, Save HTTP and private update feed.
//! Only disposable PostgreSQL /tor_ci; no outbound notifications or deploy.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
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
use tor_books::{
    repositories::{postgres::PgAuthoring, PgCatalog},
    services::{
        authoring::{new_book, save_chapter, AuthorError, SaveChapterRequest},
        chapter_file_import::parse_text,
    },
};
use tor_reading::repositories::{PgFollowUpdates, PgReader};
use tor_social::repositories::notifications::PgNotifications;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let output = Command::new("psql")
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
        .expect("isolated PostgreSQL requires psql");
    assert!(
        output.status.success(),
        "fixture SQL failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn http(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    form: &str,
    origin: &str,
) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(path);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    if method == "POST" {
        request = request
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    let reply = app
        .clone()
        .oneshot(request.body(Body::from(form.to_owned())).unwrap())
        .await
        .unwrap();
    let status = reply.status();
    let body = String::from_utf8(
        to_bytes(reply.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

#[tokio::test]
#[ignore = "requires disposable migrated PostgreSQL 18 /tor_ci"]
async fn ret01_historical_import_then_follow_sees_only_future_first_release_once() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let author = PgAuthoring::new(pool.clone());
    let updates = PgFollowUpdates::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let hashed = hash_password("RET01HistoricalImport2026").unwrap();
    let owner_token = generate_session_token();
    let reader_token = generate_session_token();
    let moderator_token = generate_session_token();
    let csrf = generate_session_token();
    let owner = accounts
        .register_with_session(
            &format!("ret01-owner-{nonce}@tor-ci.example"),
            "RET01 Owner",
            &hashed,
            &session_token_digest(&owner_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let reader = accounts
        .register_with_session(
            &format!("ret01-reader-{nonce}@tor-ci.example"),
            "RET01 Reader",
            &hashed,
            &session_token_digest(&reader_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let moderator = accounts
        .register_with_session(
            &format!("ret01-mod-{nonce}@tor-ci.example"),
            "RET01 Moderator",
            &hashed,
            &session_token_digest(&moderator_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();

    // Real B importer: while the Book is draft, even a committed batch
    // cannot create reader-visible releases or durable ChapterPublished events.
    let draft = new_book(
        &format!("tor-ci:ret01:import:{nonce}"),
        "ru",
        "RET01 Историческая книга",
        &"a".repeat(43),
    )
    .unwrap();
    let book = author.create_owned_book(owner, &draft).await.unwrap();
    let source = parse_text(
        "history.txt",
        "# Глава 1: История первая\nПрежний текст\nГлава 2 — История вторая\nДругой прежний текст",
    )
    .unwrap();
    assert_eq!(source.len(), 2);
    let import_key = "b".repeat(43);
    assert_eq!(
        author
            .import_draft_chapter_batch(owner, book, &import_key, &source)
            .await,
        Ok(2)
    );
    assert_eq!(
        author
            .import_draft_chapter_batch(owner, book, &import_key, &source)
            .await,
        Ok(2),
        "idempotent import must not create another edition"
    );
    assert_eq!(
        author
            .import_draft_chapter_batch(reader, book, &"z".repeat(43), &source)
            .await,
        Err(AuthorError::Missing),
        "reader cannot import chapters into the owner's Book"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_first_release_events e \
                 JOIN chapters c ON c.id=e.chapter_id WHERE c.book_id={book}"
            )
        ),
        "0"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events \
                 WHERE event_type='ChapterPublished' AND payload->>'book_id'='{book}'"
            )
        ),
        "0"
    );

    // The author publishes the imported Book; the live site moderator
    // must approve its pending registration before any reader can Follow.
    author.release_book(owner, book).await.unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \
             VALUES({moderator},'moderator',TRUE)"
        ),
    );
    let revision: i64 = pg(
        &url,
        &format!(
            "SELECT visibility_status_revision FROM books \
             WHERE id={book} AND visibility_status='pending_moderation'"
        ),
    )
    .parse()
    .unwrap();
    accounts
        .moderate_book_registration(
            &session_token_digest(&moderator_token),
            book,
            revision,
            "approve",
            "",
        )
        .await
        .unwrap();
    let historical = pg(
        &url,
        &format!(
            "SELECT e.chapter_id FROM chapter_first_release_events e \
             JOIN chapters c ON c.id=e.chapter_id \
             WHERE c.book_id={book} ORDER BY c.chapter_no"
        ),
    );
    let old_ids: Vec<i64> = historical.lines().map(|x| x.parse().unwrap()).collect();
    assert_eq!(
        old_ids.len(),
        2,
        "public release records both historical chapters"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events \
                 WHERE event_type='ChapterPublished' AND payload->>'book_id'='{book}' \
                   AND payload->>'first_release'='true'"
            )
        ),
        "2",
        "one initial release envelope per old chapter"
    );

    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let reader_cookie = format!("tor-dev-session={reader_token}; tor-dev-csrf={csrf}");
    let follow_url = format!("/me/books/{book}/follow");
    let follow_form = format!("csrf_token={csrf}");
    assert_eq!(
        http(
            &app,
            "POST",
            &follow_url,
            &reader_cookie,
            &follow_form,
            ORIGIN
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert!(updates.latest(reader, None).await.unwrap().items.is_empty());
    assert_eq!(updates.unread_count_capped(reader).await.unwrap(), 0);
    for chapter in &old_ids {
        assert!(
            !updates
                .set_read_state(reader, *chapter, true)
                .await
                .unwrap(),
            "historic pre-follow release cannot be acknowledged as new"
        );
    }
    let initial_updates = http(
        &app,
        "GET",
        "/me/library?view=updates",
        &reader_cookie,
        "",
        ORIGIN,
    )
    .await;
    assert_eq!(initial_updates.0, StatusCode::OK);
    assert!(!initial_updates.1.contains("RET01 New chapter"));
    assert!(!initial_updates.1.contains("История первая"));

    // An owner Save on this already-public Book creates the next FREE
    // first release. A reader's forged Save cannot publish anything.
    let create_url = format!("/me/books/{book}/chapters");
    let command_key = generate_session_token();
    let body = format!(
        "number=3&title=RET01+New+chapter&body=RET01%20future%20text&access=free\
         &price_amount=0&command_key={command_key}&csrf_token={csrf}"
    );
    let denied = http(&app, "POST", &create_url, &reader_cookie, &body, ORIGIN)
        .await
        .0;
    assert!(
        matches!(denied, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "non-owner Save must be denied: {denied}"
    );
    assert_eq!(
        http(&app, "POST", &create_url, &owner_cookie, &body, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        http(&app, "POST", &create_url, &owner_cookie, &body, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER,
        "Save replay creates no second notification or chapter"
    );
    let fresh: i64 = pg(
        &url,
        &format!("SELECT id FROM chapters WHERE book_id={book} AND chapter_no=3"),
    )
    .parse()
    .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_first_release_events e \
                 JOIN chapters c ON c.id=e.chapter_id WHERE c.book_id={book}"
            )
        ),
        "3"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events \
                 WHERE event_type='ChapterPublished' AND payload->>'book_id'='{book}'"
            )
        ),
        "3",
        "replayed Save must not create an additional ChapterPublished"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events \
                 WHERE event_type='ChapterPublished' AND aggregate_id={fresh} \
                   AND payload->>'first_release'='true' \
                   AND NOT payload ? 'body_markdown'"
            )
        ),
        "1"
    );
    let latest = updates.latest(reader, None).await.unwrap();
    assert_eq!(latest.items.len(), 1);
    assert_eq!(latest.items[0].chapter_id, fresh);
    assert_eq!(latest.items[0].chapter_number, 3);
    assert!(!latest.items[0].is_paid);
    assert!(!latest.items[0].is_read);
    assert_eq!(updates.unread_count_capped(reader).await.unwrap(), 1);
    assert!(updates
        .latest(moderator, None)
        .await
        .unwrap()
        .items
        .is_empty());

    let page = http(
        &app,
        "GET",
        "/me/library?view=updates",
        &reader_cookie,
        "",
        ORIGIN,
    )
    .await;
    assert_eq!(page.0, StatusCode::OK);
    assert!(page.1.contains("RET01 New chapter"));
    assert!(page.1.contains(&format!("href=\"/read/{fresh}\"")));
    assert!(!page.1.contains("История первая"));
    assert!(!page.1.contains("История вторая"));
    assert!(!page.1.contains("RET01 future text"));
    let home = http(&app, "GET", "/me", &reader_cookie, "", ORIGIN).await;
    assert!(home.1.contains("Новые главы подписок: 1"));

    // RET-01b: after following, editing the existing published FREE chapter
    // is a revision, not a second chapter first release. Its per-event receipt
    // is independent of reader_follow_notification_reads.
    let v2_input = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: Some(fresh as u64),
        number: 3,
        expected_revision: 1,
        title: "RET01 New chapter",
        body: "RET01 revision body V2",
        access: "free",
        price_amount: 0,
        command_key: &generate_session_token(),
    })
    .unwrap();
    let v2 = author
        .edit_published_free_chapter(owner, &v2_input)
        .await
        .unwrap();
    assert_eq!(v2.revision, 2);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_first_release_events
                 WHERE chapter_id={fresh}"
            )
        ),
        "1",
        "V2 never overwrites the first-release fact"
    );
    let bells = PgNotifications::new(pool.clone());
    // Poll the same bounded actual consumer used by the live app. The test
    // database may also have pending historical releases from this fixture.
    for _ in 0..8 {
        if bells.fanout_chapter_releases(16).await.unwrap() == 0 {
            break;
        }
    }
    for _ in 0..8 {
        if bells.deliver_pending(50).await.unwrap() == 0 {
            break;
        }
    }
    let notices = bells
        .list_section(reader, None, 30, "revisions")
        .await
        .unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].kind, "chapter_updated");
    assert!(!notices[0].is_read);
    let notice_id = notices[0].id;
    assert_eq!(
        bells.unread_chapter_revisions_capped(reader).await.unwrap(),
        1
    );
    assert_eq!(updates.unread_count_capped(reader).await.unwrap(), 1);
    let revised_page = http(
        &app,
        "GET",
        "/me/library?view=updates",
        &reader_cookie,
        "",
        ORIGIN,
    )
    .await;
    assert_eq!(revised_page.0, StatusCode::OK);
    assert!(revised_page.1.contains("<h2>Обновились</h2>"));
    assert!(revised_page
        .1
        .contains(&format!("action=\"/notifications/{notice_id}/read\"")));
    assert!(revised_page.1.contains(&format!("value=\"/read/{fresh}\"")));
    assert!(!revised_page.1.contains("RET01 revision body V2"));
    let summary = http(&app, "GET", "/me", &reader_cookie, "", ORIGIN).await;
    assert!(summary.1.contains("Обновились главы: 1"));
    let private_tab = http(
        &app,
        "GET",
        "/notifications?tab=revisions",
        &reader_cookie,
        "",
        ORIGIN,
    )
    .await;
    assert_eq!(private_tab.0, StatusCode::OK);
    assert!(private_tab.1.contains("Глава обновлена"));
    // Prove the actual desktop/mobile DOM and POST/redirect in Chromium,
    // rather than only checking an HTML string and manually posting a form.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("isolated localhost Chromium server");
    let browser_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            browser_app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let browser_result = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["--signal=TERM", "--kill-after=5s", "120s", "node"])
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tools/ret01b_revision_chromium.cjs"
            ))
            .env("RET01_BROWSER_ORIGIN", ORIGIN)
            .env("RET01_BROWSER_SESSION", reader_token)
            .env("RET01_BROWSER_CSRF", csrf)
            .env("RET01_BROWSER_CHAPTER", fresh.to_string())
            .env("RET01_BROWSER_NOTICE", notice_id.to_string())
            .output()
            .expect("Node, Playwright and Chromium installed in PG CI")
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
    assert!(
        browser_result.status.success(),
        "RET-01b Chromium failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&browser_result.stdout),
        String::from_utf8_lossy(&browser_result.stderr),
    );
    assert!(
        String::from_utf8_lossy(&browser_result.stdout)
            .contains("RET-01b real Chromium PASS: 1440/390"),
        "expected real two-viewport browser witness"
    );
    let acknowledged = bells
        .list_section(reader, None, 30, "revisions")
        .await
        .unwrap();
    assert_eq!(acknowledged.len(), 1);
    assert!(acknowledged[0].is_read);
    assert_eq!(
        bells.unread_chapter_revisions_capped(reader).await.unwrap(),
        0
    );
    assert_eq!(updates.unread_count_capped(reader).await.unwrap(), 1);
    let updated_home = http(&app, "GET", "/me", &reader_cookie, "", ORIGIN).await;
    assert!(!updated_home.1.contains("Обновились главы: 1"));

    // The history stays historical on re-follow as well: only fresh release
    // is present. A completed unfollow clears all private update visibility.
    assert_eq!(
        http(
            &app,
            "POST",
            &follow_url,
            &reader_cookie,
            &follow_form,
            ORIGIN
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(updates.latest(reader, None).await.unwrap().items.len(), 1);
    assert_eq!(
        http(
            &app,
            "POST",
            &format!("/me/books/{book}/unfollow"),
            &reader_cookie,
            &follow_form,
            ORIGIN,
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert!(updates.latest(reader, None).await.unwrap().items.is_empty());
    assert_eq!(updates.unread_count_capped(reader).await.unwrap(), 0);
    assert!(bells
        .list_section(reader, None, 30, "revisions")
        .await
        .unwrap()
        .is_empty());
    pool.close().await;
}
