//! S058: author form -> scheduled release -> private reader -> first public release.
//! All DB writes are confined to disposable /tor_ci.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use sqlx::PgPool;
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::{
    repositories::{
        postgres::{DueReleaseOutcome, PgAuthoring},
        PgCatalog,
    },
    services::authoring::{new_book, save_chapter, SaveChapterRequest},
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    body: &str,
    cookies: &str,
    origin: &str,
) -> (StatusCode, String) {
    let mut request = Request::builder().uri(path).method(method);
    if !cookies.is_empty() {
        request = request.header(header::COOKIE, cookies);
    }
    if method == "POST" {
        request = request
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_author_schedule_http_is_private_and_first_release_is_visible() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool: PgPool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let author = PgAuthoring::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("S058AuthorSchedule2026").unwrap();
    let token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let owner = accounts
        .register_with_session(
            &format!("s058-http-owner-{nonce}@tor-ci.example"),
            "Scheduled author",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let outsider = accounts
        .register_with_session(
            &format!("s058-http-outsider-{nonce}@tor-ci.example"),
            "Other user",
            &password,
            &session_token_digest(&outsider_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(owner, outsider);
    let book = new_book(
        &format!("tor-ci:s058-http:{owner}:{nonce}"),
        "ru",
        "Scheduled author book",
        &"a".repeat(43),
    )
    .unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    let first = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Chapter one",
        body: "Published first chapter",
        access: "free",
        price_amount: 0,
        command_key: &"b".repeat(43),
    })
    .unwrap();
    author.create_free_chapter(owner, &first).await.unwrap();
    author.release_book(owner, book_id).await.unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let other_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");
    let create_url = format!("/me/books/{book_id}/chapters");
    let page_url = format!("{create_url}/new");
    let (status, page) = send(&app, "GET", &page_url, "", &cookie, ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("name=\"save_as_draft\""));
    let body = format!(
        "number=2&title=Scheduled+chapter&body=PRIVATE+S058+before+due&access=free&price_amount=0&save_as_draft=1&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    assert_eq!(
        send(&app, "POST", &create_url, &body, &cookie, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let chapter = author
        .list_owned_chapters(owner, book_id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.number == 2)
        .expect("private scheduled chapter");
    let pending = author
        .pending_free_chapter(owner, book_id, chapter.id)
        .await
        .unwrap()
        .expect("private revision");
    let release_url = format!("/me/books/{book_id}/chapters/{}/release", chapter.id);
    let schedule_url = format!("/me/books/{book_id}/chapters/{}/schedule", chapter.id);
    let cancel_url = format!("/me/books/{book_id}/chapters/{}/cancel-schedule", chapter.id);
    assert_eq!(
        send(&app, "GET", &release_url, "", &other_cookie, ORIGIN)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (status, confirmation) =
        send(&app, "GET", &release_url, "", &cookie, ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(confirmation.contains("name=\"scheduled_utc\""));
    assert!(confirmation.contains("PRIVATE S058 before due"));

    let utc: String = sqlx::query_scalar(
        "SELECT to_char((now()+interval '1 hour') AT TIME ZONE 'UTC',
                'YYYY-MM-DD\"T\"HH24:MI')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let scheduled = format!(
        "expected_revision={}&version_id={}&scheduled_utc={}&csrf_token={csrf}",
        pending.revision,
        pending.version_id,
        utc.replace(':', "%3A")
    );
    assert_eq!(
        send(&app, "POST", &schedule_url, &scheduled, &other_cookie, ORIGIN)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &schedule_url,
            &scheduled,
            &cookie,
            "https://attacker.invalid"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let invalid_csrf = scheduled.replace(&format!("csrf_token={csrf}"), "csrf_token=wrong");
    assert_eq!(
        send(&app, "POST", &schedule_url, &invalid_csrf, &cookie, ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &schedule_url, &scheduled, &cookie, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let (_, scheduled_page) = send(&app, "GET", &release_url, "", &cookie, ORIGIN).await;
    assert!(scheduled_page.contains(&utc));
    assert!(scheduled_page.contains("pending"));
    let reader_path = format!("/reader/{}", chapter.id);
    assert_eq!(
        send(&app, "GET", &reader_path, "", "", ORIGIN).await.0,
        StatusCode::NOT_FOUND,
        "scheduled text must remain private before the deadline"
    );
    let (_, public_book) = send(
        &app,
        "GET",
        &format!("/books/{book_id}"),
        "",
        "",
        ORIGIN,
    )
    .await;
    assert!(!public_book.contains("Scheduled chapter"));

    let utc2: String = sqlx::query_scalar(
        "SELECT to_char((now()+interval '2 hours') AT TIME ZONE 'UTC',
                'YYYY-MM-DD\"T\"HH24:MI')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let rescheduled = scheduled.replace(&utc.replace(':', "%3A"), &utc2.replace(':', "%3A"));
    assert_eq!(
        send(&app, "POST", &schedule_url, &rescheduled, &cookie, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let (_, rescheduled_page) = send(&app, "GET", &release_url, "", &cookie, ORIGIN).await;
    assert!(rescheduled_page.contains(&utc2));
    assert_eq!(
        send(
            &app,
            "POST",
            &cancel_url,
            &format!("csrf_token={csrf}"),
            &cookie,
            ORIGIN
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let (_, cancelled_page) = send(&app, "GET", &release_url, "", &cookie, ORIGIN).await;
    assert!(cancelled_page.contains("cancelled"));
    assert_eq!(author.process_one_due_scheduled_release().await.unwrap(), None);
    assert_eq!(
        send(&app, "POST", &schedule_url, &scheduled, &cookie, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    sqlx::query(
        "UPDATE chapter_scheduled_releases SET scheduled_at=now()-interval '1 second'
         WHERE chapter_id=$1",
    )
    .bind(chapter.id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        author.process_one_due_scheduled_release().await.unwrap(),
        Some((chapter.id, DueReleaseOutcome::Published))
    );
    let (status, visible) = send(&app, "GET", &reader_path, "", "", ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(visible.contains("PRIVATE S058 before due"));
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1",
    )
    .bind(chapter.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1, "one reader-visible first-release event");


    // A held book cannot be published by a previously valid schedule.
    let third = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 3,
        expected_revision: 0,
        title: "Held chapter",
        body: "Never publish while book is held",
        access: "free",
        price_amount: 0,
        command_key: &"d".repeat(43),
    })
    .unwrap();
    let saved3 = author
        .create_pending_free_chapter(owner, &third)
        .await
        .unwrap();
    author
        .schedule_pending_free_chapter(
            owner,
            book_id,
            saved3.chapter_id,
            1,
            saved3.version_id,
            &utc,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE chapter_scheduled_releases SET scheduled_at=now()-interval '1 second'
         WHERE chapter_id=$1",
    )
    .bind(saved3.chapter_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        author.process_one_due_scheduled_release().await.unwrap(),
        Some((saved3.chapter_id, DueReleaseOutcome::Stale))
    );
    let held_publications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_publications WHERE chapter_id=$1")
            .bind(saved3.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(held_publications, 0);
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();

    // Changing the book owner after scheduling invalidates the pinned actor.
    let fourth = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 4,
        expected_revision: 0,
        title: "Transferred book chapter",
        body: "Old owner schedule must not publish",
        access: "free",
        price_amount: 0,
        command_key: &"e".repeat(43),
    })
    .unwrap();
    let saved4 = author
        .create_pending_free_chapter(owner, &fourth)
        .await
        .unwrap();
    author
        .schedule_pending_free_chapter(
            owner,
            book_id,
            saved4.chapter_id,
            1,
            saved4.version_id,
            &utc,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE books SET owner_user_id=$2 WHERE id=$1")
        .bind(book_id)
        .bind(outsider)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE chapter_scheduled_releases SET scheduled_at=now()-interval '1 second'
         WHERE chapter_id=$1",
    )
    .bind(saved4.chapter_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        author.process_one_due_scheduled_release().await.unwrap(),
        Some((saved4.chapter_id, DueReleaseOutcome::Stale))
    );
    let transferred_publications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_publications WHERE chapter_id=$1")
            .bind(saved4.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(transferred_publications, 0);

    pool.close().await;
}
