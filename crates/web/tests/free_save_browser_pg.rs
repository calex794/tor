//! Real HTTP owner Save in a published book must immediately expose one
//! immutable free chapter, without a second confirmation request.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{env, num::NonZeroU32, process::Command};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_ingest::{
    repositories::{notify::PgNotificationRecipients, outbox::PgOutbox},
    services::notify_worker::{NotificationRun, NotificationWorker},
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
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
        .expect("PostgreSQL client is required for isolated PG fixture");
    assert!(
        result.status.success(),
        "fixture failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn cookies(session: &str, csrf: &str) -> String {
    format!("tor-dev-session={session}; tor-dev-csrf={csrf}")
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    body: &str,
    session: Option<&str>,
    csrf: &str,
    origin: &str,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(session) = session {
        request = request.header(header::COOKIE, cookies(session, csrf));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
#[ignore = "requires isolated PG18 tor_ci"]
async fn browser_save_free_publishes_without_separate_release_and_replay_is_safe() {
    let url = env::var("TEST_DATABASE_URL").expect("PG18 fixture");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let reader = PgReader::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    let token = generate_session_token();
    let password = hash_password("FreeChapterBrowser2026").unwrap();
    let owner = accounts
        .register_with_session(
            "free-browser-owner@example.invalid",
            "Free browser author",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let follower_token = generate_session_token();
    let follower = accounts
        .register_with_session(
            "free-browser-follower@example.invalid",
            "Free browser follower",
            &password,
            &session_token_digest(&follower_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:free-browser-save','ru','Free Browser','public',{owner}) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let csrf = generate_session_token();
    let command = generate_session_token();
    let (status, _) = send(
        &app,
        "POST",
        &format!("/me/books/{book}/follow"),
        &format!("csrf_token={csrf}"),
        Some(&follower_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "reader follows before release"
    );
    let path = format!("/me/books/{book}/chapters");
    let page_path = format!("/me/books/{book}/chapters/new");
    let (status, html) = send(&app, "GET", &page_path, "", Some(&token), &csrf, ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("сразу после сохранения"));
    assert!(!html.contains("отдельное подтверждение"));
    let body = format!(
        "number=2&title=New+chapter&body=Immediate%20free%20text&access=free&price_amount=0&command_key={command}&csrf_token={csrf}"
    );
    assert_eq!(
        send(&app, "POST", &path, &body, None, &csrf, ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &path,
            &body,
            Some(&token),
            &csrf,
            "https://evil.example"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let malformed = body.replace(&format!("csrf_token={csrf}"), "csrf_token=wrong");
    assert_eq!(
        send(&app, "POST", &path, &malformed, Some(&token), &csrf, ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &path, &body, Some(&token), &csrf, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "POST", &path, &body, Some(&token), &csrf, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER,
        "same Save can safely replay"
    );
    let altered = body.replace("Immediate%20free%20text", "Changed%20text");
    assert_eq!(
        send(&app, "POST", &path, &altered, Some(&token), &csrf, ORIGIN)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let row = pg(
        &url,
        &format!(
            "SELECT c.id || '|' || c.visibility || '|' || p.version_id
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.book_id={book}"
        ),
    );
    let mut columns = row.split('|');
    let chapter_id: i64 = columns.next().unwrap().parse().unwrap();
    assert_eq!(columns.next(), Some("public"));
    let version_id: i64 = columns.next().unwrap().parse().unwrap();
    assert_eq!(columns.next(), None);
    let event = pg(
        &url,
        &format!(
            "SELECT count(*) FROM chapter_first_release_events
             WHERE chapter_id={chapter_id} AND version_id={version_id}"
        ),
    );
    assert_eq!(event, "1");
    // The B command must commit its outbox envelope with publication; a
    // real G worker claims it, stages the subscriber, and leaves independent
    // search projection pending rather than pretending it is complete.
    let event_id = pg(
        &url,
        &format!(
            "SELECT id::text FROM outbox_events
             WHERE event_type='ChapterPublished' AND aggregate_id={chapter_id}
             AND (payload->>'version_id')::bigint={version_id}"
        ),
    );
    assert_eq!(event_id.len(), 36);
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET next_attempt_at=now()-interval '20 days'
             WHERE id='{event_id}'::uuid"
        ),
    );
    let notify = NotificationWorker::new(
        PgOutbox::new(pool.clone()),
        PgNotificationRecipients::new(pool.clone()),
    );
    assert_eq!(
        notify.run_one().await.unwrap(),
        NotificationRun::Staged {
            recipients: 1,
            pages: 1,
        }
    );
    let intent_count = pg(
        &url,
        &format!(
            "SELECT count(*) FROM notification_event_recipient_intents
             WHERE event_id='{event_id}'::uuid AND user_id={follower}"
        ),
    );
    assert_eq!(
        intent_count, "1",
        "real HTTP follower has one staged intent"
    );
    let delivery_count = pg(
        &url,
        &format!(
            "SELECT count(*) FROM event_deliveries
             WHERE event_id='{event_id}'::uuid AND consumer='search-index-v1'"
        ),
    );
    assert_eq!(delivery_count, "0", "G must not acknowledge F's work");
    let (status, follower_html) = send(
        &app,
        "GET",
        "/me/library",
        "",
        Some(&follower_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        follower_html.contains("New chapter"),
        "followed reader must see the newly published chapter in C-16"
    );
    assert!(!follower_html.contains("Immediate free text"));

    let (status, reader_html) = send(
        &app,
        "GET",
        &format!("/reader/{chapter_id}"),
        "",
        None,
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(reader_html.contains("Immediate free text"));

    // BKR-96: the same public chapter can be edited from the owner's
    // existing form. Saving V2 immediately changes the reader pointer.
    let edit_path = format!("/me/books/{book}/chapters/{chapter_id}/edit");
    let (status, editor) = send(&app, "GET", &edit_path, "", Some(&token), &csrf, ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(editor.contains("Сохранение сразу обновит опубликованную редакцию"));
    assert!(editor.contains("name=\"expected_revision\" value=\"1\""));
    let edit_key = editor
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .expect("owner edit key")
        .split('"')
        .next()
        .unwrap();
    let edited = format!(
        "number=2&title=Updated+chapter&body=Updated%20free%20text%20V2&expected_revision=1&command_key={edit_key}&csrf_token={csrf}"
    );
    assert_eq!(
        send(&app, "POST", &edit_path, &edited, None, &csrf, ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &edit_path,
            &edited,
            Some(&token),
            &csrf,
            "https://evil.example",
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    let invalid_csrf = edited.replace(&format!("csrf_token={csrf}"), "csrf_token=wrong");
    assert_eq!(
        send(
            &app,
            "POST",
            &edit_path,
            &invalid_csrf,
            Some(&token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &edit_path,
            &edited,
            Some(&token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &edit_path,
            &edited,
            Some(&token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::SEE_OTHER,
        "same author Save may replay without another version",
    );
    let stale = edited.replace(edit_key, &"s".repeat(43));
    assert_eq!(
        send(
            &app,
            "POST",
            &edit_path,
            &stale,
            Some(&token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "a stale edit form cannot replace a new published version",
    );
    let (status, new_reader) = send(
        &app,
        "GET",
        &format!("/reader/{chapter_id}"),
        "",
        None,
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(new_reader.contains("Updated free text V2"));
    assert!(!new_reader.contains("Immediate free text"));
    // New V2 exists only in the G-08 committed revision ledger, not the
    // C-16 first-release feed. The worker stages it and the follower views it.
    let v2_id = pg(
        &url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter_id}"),
    );
    assert_ne!(v2_id, version_id.to_string());
    let v2_event = pg(
        &url,
        &format!(
            "SELECT id::text FROM outbox_events
             WHERE event_type='ChapterPublished' AND aggregate_id={chapter_id}
             AND (payload->>'version_id')::bigint={v2_id}"
        ),
    );
    assert_eq!(v2_event.len(), 36);
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET next_attempt_at=now()-interval '19 days'
             WHERE id='{v2_event}'::uuid"
        ),
    );
    assert_eq!(
        notify.run_one().await.unwrap(),
        NotificationRun::Staged {
            recipients: 1,
            pages: 1,
        },
    );
    let (status, updated_library) = send(
        &app,
        "GET",
        "/me/library",
        "",
        Some(&follower_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(updated_library.contains("Обновлённые редакции"));
    assert!(updated_library.contains("Updated chapter"));
    assert!(updated_library.contains("V2"));
    assert!(!updated_library.contains("Updated free text V2"));

    let action = format!("/me/revisions/{v2_event}/read-state");
    assert!(
        updated_library.contains(&action),
        "V2 has an explicit read control"
    );
    let read_form = format!("state=read&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", &action, &read_form, None, &csrf, ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &action,
            &read_form,
            Some(&follower_token),
            &csrf,
            "https://evil.example"
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &action,
            "state=read&csrf_token=bad",
            Some(&follower_token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &action,
            "state=invalid",
            Some(&follower_token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &action,
            &read_form,
            Some(&token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "the author is not a subscriber",
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &action,
            &read_form,
            Some(&follower_token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &action,
            &read_form,
            Some(&follower_token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::SEE_OTHER,
        "repeating mark-read is idempotent",
    );
    let (status, seen) = send(
        &app,
        "GET",
        "/me/library",
        "",
        Some(&follower_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(seen.contains("Просмотрено"));
    assert!(seen.contains("name=\"state\" value=\"unread\""));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT read_at IS NOT NULL FROM notification_event_recipient_intents
             WHERE event_id='{v2_event}'::uuid AND user_id={follower}"
            )
        ),
        "t",
    );
    let unread_form = format!("state=unread&csrf_token={csrf}");
    assert_eq!(
        send(
            &app,
            "POST",
            &action,
            &unread_form,
            Some(&follower_token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT read_at IS NULL FROM notification_event_recipient_intents
             WHERE event_id='{v2_event}'::uuid AND user_id={follower}"
            )
        ),
        "t",
    );
    let (status, unread_page) = send(
        &app,
        "GET",
        "/me/library",
        "",
        Some(&follower_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(unread_page.contains("name=\"state\" value=\"read\""));
    assert_eq!(
        send(
            &app,
            "POST",
            "/me/revisions/bad-uuid/read-state",
            &read_form,
            Some(&follower_token),
            &csrf,
            ORIGIN
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
    );

    let (status, owner_library) =
        send(&app, "GET", "/me/library", "", Some(&token), &csrf, ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !owner_library.contains("Обновлённые редакции"),
        "the author is not subscribed to this publication",
    );
    let (bad_cursor, _) = send(
        &app,
        "GET",
        "/me/library?revisions_after=0.invalid",
        "",
        Some(&follower_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(bad_cursor, StatusCode::BAD_REQUEST);

    let immutable_v1 = pg(
        &url,
        &format!("SELECT body_markdown FROM chapter_versions WHERE id={version_id}"),
    );
    assert_eq!(immutable_v1, "Immediate free text");
    let version_count = pg(
        &url,
        &format!("SELECT count(*) FROM chapter_versions WHERE chapter_id={chapter_id}"),
    );
    assert_eq!(version_count, "2");
    pool.close().await;
}
