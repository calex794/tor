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
use tor_discussions::repositories::public_chapter_comments::{
    CreateResult as CommentCreateResult, NewChapterComment, PgPublicChapterComments,
};
use tor_reading::repositories::PgReader;
use tor_social::repositories::notifications::PgNotifications;
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
        .env("PGCONNECT_TIMEOUT", "5")
        .env("PGOPTIONS", "-c statement_timeout=15000")
        .output()
        .expect("PostgreSQL client is required for isolated PG fixture");
    if !result.status.success() {
        // Observe the *other* transaction still holding the book FK's
        // KEY SHARE conflict; never disable the constraint or retry blindly.
        let diagnosis = Command::new("psql")
            .args([
                "-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1",
                "--dbname", url, "--command",
                "SELECT pid::text || ' | ' || coalesce(state,'') || ' | ' ||
                        coalesce(wait_event_type,'') || ':' || coalesce(wait_event,'') ||
                        ' | xact_age=' || coalesce((clock_timestamp()-xact_start)::text,'') ||
                        ' | blockers=' || pg_blocking_pids(pid)::text || ' | ' ||
                        left(replace(query,chr(10),' '),240)
                 FROM pg_stat_activity
                 WHERE datname=current_database() AND pid<>pg_backend_pid()
                 ORDER BY pid",
            ])
            .env("PGCONNECT_TIMEOUT", "5")
            .output();
        let activity = diagnosis.as_ref().map(|out| {
            format!(
                "stdout: {}; stderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        panic!(
            "isolated PG fixture failed: {}\nPostgreSQL active transactions: {}",
            String::from_utf8_lossy(&result.stderr),
            activity.unwrap_or_else(|error| format!("diagnostics unavailable: {error}"))
        );
    }
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
        .header(
            header::CONTENT_TYPE,
            if method == "PUT" {
                "application/json"
            } else {
                "application/x-www-form-urlencoded"
            },
        );
    if method == "PUT" {
        request = request.header("x-csrf-token", csrf);
    }
    if let Some(session) = session {
        request = request.header(header::COOKIE, cookies(session, csrf));
    }
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        app.clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap()),
    )
    .await
    .expect("free-save HTTP handler stalled for more than 20 seconds")
    .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn assert_age_comment_page(
    app: &Router,
    chapter: i64,
    session: Option<&str>,
    csrf: &str,
    expected: StatusCode,
    readable: bool,
) {
    let (status, html) = send(
        app,
        "GET",
        &format!("/reader/{chapter}"),
        "",
        session,
        csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, expected, "age-gated reader HTTP status");
    assert_eq!(html.contains("S020 age-scoped comment"), readable);
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
    let reader_token = generate_session_token();
    let reader_user = accounts
        .register_with_session(
            "free-browser-sync-reader@example.invalid",
            "Free browser sync reader",
            &password,
            &session_token_digest(&reader_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:free-browser-save','ru','Free Browser','public',{owner},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let notify_reader: i64 = pg(
        &url,
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-browser-g08-follower@tor-ci.example','G08 reader','$argon2id$test')
         RETURNING id",
    )
    .parse()
    .unwrap();
    let notify_optout: i64 = pg(
        &url,
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-browser-g08-optout@tor-ci.example','G08 optout','$argon2id$test')
         RETURNING id",
    )
    .parse()
    .unwrap();
    let notify_late: i64 = pg(
        &url,
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-browser-g08-late@tor-ci.example','G08 late','$argon2id$test')
         RETURNING id",
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO reader_book_follows(user_id,book_id)
             VALUES({notify_reader},{book}),({notify_optout},{book})"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO user_notification_preferences(user_id,kind,enabled)
             VALUES({notify_optout},'chapter_created',false)"
        ),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let csrf = generate_session_token();
    let command = generate_session_token();
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
    // G-06/G-08: assert the real authenticated HTTP Save creates an outbox
    // root that the *existing* S058 reader fanout and E-08 on-site consumer
    // can deliver after a simulated worker restart.
    let root: String = pg(
        &url,
        &format!(
            "SELECT id::text FROM outbox_events
             WHERE event_type='ChapterPublished' AND aggregate_id={chapter_id}
               AND payload->>'version_id'='{version_id}'"
        ),
    );
    assert!(
        !root.is_empty(),
        "HTTP Save must write the durable root event"
    );
    pg(
        &url,
        &format!(
            "INSERT INTO reader_book_follows(user_id,book_id)
             VALUES({notify_late},{book})"
        ),
    );
    eprintln!("G08_E2E_STAGE: root recorded; starting fanout");
    let notifications = PgNotifications::new(pool.clone());
    let mut root_done = false;
    for _ in 0..12 {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            notifications.fanout_chapter_releases(100),
        )
        .await
        .expect("G08 fanout exceeded 20 seconds")
        .unwrap();
        root_done = pg(
            &url,
            &format!(
                "SELECT delivered_at IS NOT NULL FROM outbox_events
                 WHERE id='{root}'::uuid"
            ),
        ) == "t";
        if root_done {
            break;
        }
    }
    assert!(root_done, "the real root must be processed by S058");
    let children = pg(
        &url,
        &format!(
            "SELECT count(*) FROM outbox_events
             WHERE event_type='reader.chapter.released' AND aggregate_id={chapter_id}"
        ),
    );
    assert_eq!(children, "2", "only the original followers are eligible");
    drop(notifications);
    eprintln!("G08_E2E_STAGE: root fanout done; starting delivery");
    let after_restart = PgNotifications::new(pool.clone());
    let mut child_receipts = String::new();
    for _ in 0..12 {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            after_restart.deliver_pending(100),
        )
        .await
        .expect("G08 delivery exceeded 20 seconds")
        .unwrap();
        child_receipts = pg(
            &url,
            &format!(
                "SELECT count(*) FROM event_deliveries d
                 JOIN outbox_events e ON e.id=d.event_id
                 WHERE e.event_type='reader.chapter.released'
                   AND e.aggregate_id={chapter_id}
                   AND d.consumer='user_notifications'"
            ),
        );
        if child_receipts == "2" {
            break;
        }
    }
    assert_eq!(child_receipts, "2", "each recipient event must be acked");
    for (recipient, expected) in [
        (notify_reader, "1"),
        (notify_optout, "0"),
        (notify_late, "0"),
    ] {
        let actual = pg(
            &url,
            &format!(
                "SELECT count(*) FROM user_notifications
                 WHERE recipient_user_id={recipient}
                   AND kind='chapter_created' AND url='/reader/{chapter_id}'"
            ),
        );
        assert_eq!(actual, expected, "notification eligibility and opt-out");
    }
    eprintln!("G08_E2E_STAGE: recipient delivery completed; testing replay");
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        after_restart.deliver_pending(8),
    )
    .await
    .expect("G08 replay exceeded 20 seconds")
    .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM user_notifications
                 WHERE recipient_user_id={notify_reader}
                   AND kind='chapter_created' AND url='/reader/{chapter_id}'"
            )
        ),
        "1",
        "replay cannot duplicate a delivered on-site notification"
    );
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

    // S020/#486: a signed-in reader with the current 18+/21+ permission can
    // sync a free chapter. Anonymous public_chapter() would incorrectly 404.
    let sync_get = format!("/api/v1/me/progress/free?chapter_id={chapter_id}");
    let sync_put = "/api/v1/me/progress/free";
    let current_version: i64 = pg(
        &url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter_id}"),
    )
    .parse()
    .unwrap();
    // S020 E-02: the very same public-reader HTML must enforce live age
    // rights before rendering canonical chapter comment text.
    let comment_key = generate_session_token();
    let comments = PgPublicChapterComments::new(pool.clone());
    assert!(matches!(
        comments
            .create_free(
                owner,
                NewChapterComment {
                    chapter_id,
                    version_id: current_version,
                    body: "S020 age-scoped comment",
                    command_key: &comment_key,
                    payload_hash: &[19; 32],
                    parent_id: None,
                    anchor: None,
                },
            )
            .await,
        CommentCreateResult::Created(_)
    ));
    assert_age_comment_page(&app, chapter_id, None, &csrf, StatusCode::OK, true).await;
    let sync_body = |revision: i64, offset: i32| {
        format!(
            r#"{{"chapter_id":"{chapter_id}","version_id":"{current_version}","block_id":1,"char_offset":{offset},"expected_revision":{revision}}}"#
        )
    };
    assert_eq!(
        send(&app, "GET", &sync_get, "", None, &csrf, ORIGIN)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    pg(
        &url,
        &format!("UPDATE books SET access_level='adult' WHERE id={book}"),
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &sync_get,
            "",
            Some(&reader_token),
            &csrf,
            ORIGIN,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_age_comment_page(
        &app,
        chapter_id,
        Some(&reader_token),
        &csrf,
        StatusCode::NOT_FOUND,
        false,
    )
    .await;
    pg(
        &url,
        &format!("UPDATE users SET adult_enabled=TRUE WHERE id={reader_user}"),
    );
    assert_age_comment_page(
        &app,
        chapter_id,
        Some(&reader_token),
        &csrf,
        StatusCode::OK,
        true,
    )
    .await;
    assert_eq!(
        send(
            &app,
            "GET",
            &sync_get,
            "",
            Some(&reader_token),
            &csrf,
            ORIGIN,
        )
        .await
        .0,
        StatusCode::OK,
        "an eligible adult must be able to retrieve free sync state"
    );
    let (status, saved) = send(
        &app,
        "PUT",
        sync_put,
        &sync_body(0, 1),
        Some(&reader_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(saved.contains("\"revision\":1"));

    pg(
        &url,
        &format!("UPDATE books SET access_level='pro_adult' WHERE id={book}"),
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &sync_get,
            "",
            Some(&reader_token),
            &csrf,
            ORIGIN,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_age_comment_page(
        &app,
        chapter_id,
        Some(&reader_token),
        &csrf,
        StatusCode::NOT_FOUND,
        false,
    )
    .await;
    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({reader_user},'pro_adult','moderation',TRUE)"
        ),
    );
    assert_age_comment_page(
        &app,
        chapter_id,
        Some(&reader_token),
        &csrf,
        StatusCode::OK,
        true,
    )
    .await;
    let (status, saved) = send(
        &app,
        "PUT",
        sync_put,
        &sync_body(1, 2),
        Some(&reader_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(saved.contains("\"revision\":2"));
    pg(
        &url,
        &format!("UPDATE user_level_grants SET is_active=FALSE WHERE user_id={reader_user}"),
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &sync_get,
            "",
            Some(&reader_token),
            &csrf,
            ORIGIN,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_age_comment_page(
        &app,
        chapter_id,
        Some(&reader_token),
        &csrf,
        StatusCode::NOT_FOUND,
        false,
    )
    .await;
    pg(
        &url,
        &format!("UPDATE books SET access_level=NULL WHERE id={book}"),
    );
    assert_age_comment_page(
        &app,
        chapter_id,
        Some(&reader_token),
        &csrf,
        StatusCode::NOT_FOUND,
        false,
    )
    .await;
    assert_eq!(
        send(
            &app,
            "GET",
            &sync_get,
            "",
            Some(&reader_token),
            &csrf,
            ORIGIN,
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "legacy NULL tier never inherits an adult session"
    );

    // NEwBOO parity: unlisting by pending_deletion does not itself revoke
    // existing direct free-reading access. Site blocking does.
    pg(
        &url,
        &format!(
            "UPDATE books SET access_level='basic',lifecycle_status='pending_deletion' WHERE id={book}"
        ),
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &sync_get,
            "",
            Some(&reader_token),
            &csrf,
            ORIGIN,
        )
        .await
        .0,
        StatusCode::OK
    );
    pg(
        &url,
        &format!("UPDATE books SET visibility_status='blocked' WHERE id={book}"),
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &sync_get,
            "",
            Some(&reader_token),
            &csrf,
            ORIGIN,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    pool.close().await;
}
