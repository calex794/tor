//! C-15: authenticated demand and owner aggregate via real isolated PostgreSQL.
#![forbid(unsafe_code)]
#[path = "support/ret02_return_chromium.rs"]
mod ret02_return_chromium;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tor_social::repositories::notifications::PgNotifications;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

/// Keep SQLx outside tor-web. These are test-only fixtures using the test role.
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
        "fixture SQL failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookies: &str,
    origin: &str,
    body: &str,
) -> Response {
    let request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .expect("HTTP request");
    app.clone().oneshot(request).await.expect("HTTP response")
}

async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("HTML body")
            .to_vec(),
    )
    .expect("UTF-8")
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn c_15_http_demand_requires_completion_csrf_and_owner_scoping() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    let catalog = PgCatalog::connect(&url).await.expect("test pool");
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let owner_session = generate_session_token();
    let reader_session = generate_session_token();
    let observer_session = generate_session_token();
    let csrf = generate_session_token();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password_hash = hash_password("C15ContinuationBrowser2026").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("c15-owner-{nonce}@tor-ci.example"),
            "Author",
            &password_hash,
            &session_token_digest(&owner_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let reader = accounts
        .register_with_session(
            &format!("c15-reader-{nonce}@tor-ci.example"),
            "Reader",
            &password_hash,
            &session_token_digest(&reader_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let observer = accounts
        .register_with_session(
            &format!("c15-observer-{nonce}@tor-ci.example"),
            "Observer",
            &password_hash,
            &session_token_digest(&observer_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();

    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:c15:http:{nonce}','ru','Книга для читателей','public',{owner},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Глава 1','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Первая глава') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );

    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let owner_cookie = format!("tor-dev-session={owner_session}; tor-dev-csrf={csrf}");
    let reader_cookie = format!("tor-dev-session={reader_session}; tor-dev-csrf={csrf}");
    let observer_cookie = format!("tor-dev-session={observer_session}; tor-dev-csrf={csrf}");
    let read_path = format!("/read/{chapter}");
    let want_path = format!("/read/{chapter}/want-continuation");
    let owner_path = format!("/me/books/{book}");
    let form = format!("csrf_token={csrf}");

    let page = call(&app, "GET", &read_path, &reader_cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(!html(page).await.contains("Хочу продолжение"));
    assert_eq!(
        call(&app, "POST", &want_path, &reader_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::CONFLICT,
        "opened chapter is not a trusted completion"
    );

    // First meaningful read must be created by the same real HTTP completion
    // path as a returning reader; direct fixture SQL would hide a broken entry.
    let first_finish = format!(
        "csrf_token={csrf}&version_id={version}&block_index=1&char_offset={}&expected_revision=0",
        "Первая глава".encode_utf16().count()
    );
    let first_completion = call(
        &app,
        "POST",
        &format!("/read/{chapter}/complete"),
        &reader_cookie,
        ORIGIN,
        &first_finish,
    )
    .await;
    assert_eq!(first_completion.status(), StatusCode::SEE_OTHER);
    let first_return = format!("/read/{chapter}#continuation");
    assert_eq!(
        first_completion
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some(first_return.as_str()),
        "completion must return reader to the continuation action, not rankings"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions \
                 WHERE user_id={reader} AND chapter_id={chapter}"
            )
        ),
        "1",
        "initial completion must be persisted before demand is enabled"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events \
                 WHERE event_type='ChapterCompletedV1' \
                   AND dedupe_key='chapter-completed:{reader}:{chapter}'"
            )
        ),
        "1"
    );

    let ready = html(call(&app, "GET", &read_path, &reader_cookie, ORIGIN, "").await).await;
    assert!(ready.contains("Хочу продолжение"));
    assert!(ready.contains("Мои рейтинги"));
    assert!(!ready.contains("user_id"));
    assert_eq!(
        call(
            &app,
            "POST",
            &want_path,
            &reader_cookie,
            "https://evil.invalid",
            &form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &want_path,
            &reader_cookie,
            ORIGIN,
            "csrf_token=invalid"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", &want_path, "", ORIGIN, &form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &want_path, &reader_cookie, ORIGIN, &form)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    let count: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM reader_continuation_demand
             WHERE user_id={reader} AND book_id={book} AND frontier_chapter_id={chapter}"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(count, 1, "duplicate requests must be idempotent");
    let confirmed = html(call(&app, "GET", &read_path, &reader_cookie, ORIGIN, "").await).await;
    assert!(confirmed.contains("Вы уже сообщили автору"));
    // After a verified completion with no next release, the library must
    // still point to the completed frontier rather than invent a chapter.
    let waiting_library = html(
        call(
            &app,
            "GET",
            "/me/library?view=reading",
            &reader_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(waiting_library.contains(&format!("href=\"/read/{chapter}#p-1\"")));

    assert_eq!(
        call(&app, "GET", &owner_path, &reader_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "reader must not see the author's demand counter"
    );
    let author = html(call(&app, "GET", &owner_path, &owner_cookie, ORIGIN, "").await).await;
    assert!(author.contains("Запросов после последней опубликованной главы: <strong>1</strong>"));
    assert!(!author.contains(&format!("c15-reader-{nonce}@tor-ci.example")));

    // C-15/B-06: close the minimal on-site loop via the actual author Save,
    // rather than only fixture SQL. External email/push is out of scope.
    let follow_path = format!("/me/books/{book}/follow");
    assert_eq!(
        call(&app, "POST", &follow_path, &reader_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let save_path = format!("/me/books/{book}/chapters");
    let save_body = format!(
        "number=2&title=Demand+fulfilled&body=C15%20reader%20return&access=free&price_amount=0&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    let denied = call(&app, "POST", &save_path, &reader_cookie, ORIGIN, &save_body)
        .await
        .status();
    assert!(
        matches!(denied, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "a reader must not publish the requested continuation: {denied}"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapters WHERE book_id={book} AND chapter_no=2")
        ),
        "0"
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &save_path, &owner_cookie, ORIGIN, &save_body)
                .await
                .status(),
            StatusCode::SEE_OTHER,
            "author Save must publish once and replay idempotently"
        );
    }
    let released: i64 = pg(
        &url,
        &format!("SELECT id FROM chapters WHERE book_id={book} AND chapter_no=2"),
    )
    .parse()
    .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id={released}"
            )
        ),
        "1",
        "one author Save must create exactly one reader-visible first release"
    );
    let follower_updates = html(
        call(
            &app,
            "GET",
            "/me/library?view=updates",
            &reader_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(follower_updates.contains("Demand fulfilled"));
    assert!(follower_updates.contains(&format!("href=\"/read/{released}\"")));
    assert!(!follower_updates.contains(&format!("href=\"/reader/{released}\"")));
    assert!(follower_updates.contains(&format!("/me/updates/{released}/read-state")));
    assert!(follower_updates.contains("Отметить прочитанной"));

    // E-08 must project exactly one first-release bell for A and none for B.
    // This uses the actual bounded worker, not a fabricated per-user SQL row.
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events
                 WHERE event_type='ChapterPublished' AND aggregate_id={released}
                   AND payload->>'first_release'='true'"
            )
        ),
        "1",
        "the real author Save produces one immutable release envelope"
    );
    let notices = PgNotifications::new(pool.clone());
    // RET-02b: a fenced root from an interrupted worker cannot fan out.
    // Both the lease and retry deadline must mature before projection.
    let release_root = format!(
        "event_type='ChapterPublished' AND aggregate_id={released}
         AND payload->>'first_release'='true'"
    );
    pg(
        &url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()+interval '10 minutes',
                 next_attempt_at=clock_timestamp()-interval '1 second'
             WHERE {release_root}"
        ),
    );
    let _ = notices.fanout_chapter_releases(32).await.unwrap();
    let release_children = || {
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events
                 WHERE event_type='reader.chapter.released' AND aggregate_id={released}"
            ),
        )
    };
    assert_eq!(release_children(), "0", "an active lease blocks fanout");
    pg(
        &url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()-interval '1 second',
                 next_attempt_at=clock_timestamp()+interval '10 minutes'
             WHERE {release_root}"
        ),
    );
    let _ = notices.fanout_chapter_releases(32).await.unwrap();
    assert_eq!(release_children(), "0", "retry deadline blocks premature fanout");
    pg(
        &url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()-interval '1 second',
                 next_attempt_at=clock_timestamp()-interval '1 second'
             WHERE {release_root}"
        ),
    );
    for _ in 0..8 {
        if notices.fanout_chapter_releases(32).await.unwrap() == 0 {
            break;
        }
    }
    // E-08 child delivery has its own fenced lease and independent retry.
    // A root release must never become a duplicate personal bell on replay.
    let release_child = format!(
        "event_type='reader.chapter.released' AND aggregate_id={released}
         AND payload->>'recipient_user_id'='{reader}'"
    );
    assert_eq!(
        pg(&url, &format!("SELECT count(*) FROM outbox_events WHERE {release_child}")),
        "1",
        "one child event was created for the follower"
    );
    let delivered_bells = || {
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM user_notifications n
                 JOIN outbox_events e ON e.id=n.source_event_id
                 WHERE {release_child}"
            ),
        )
    };
    pg(
        &url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()+interval '10 minutes',
                 next_attempt_at=clock_timestamp()-interval '1 second'
             WHERE {release_child}"
        ),
    );
    let _ = notices.deliver_pending(64).await.unwrap();
    assert_eq!(delivered_bells(), "0", "a fenced child cannot notify");
    pg(
        &url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()-interval '1 second',
                 next_attempt_at=clock_timestamp()+interval '10 minutes'
             WHERE {release_child}"
        ),
    );
    let _ = notices.deliver_pending(64).await.unwrap();
    assert_eq!(delivered_bells(), "0", "a deferred child cannot notify early");
    pg(
        &url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()-interval '1 second',
                 next_attempt_at=clock_timestamp()-interval '1 second'
             WHERE {release_child}"
        ),
    );
    for _ in 0..8 {
        if notices.deliver_pending(64).await.unwrap() == 0 {
            break;
        }
    }
    // Repeated fanout and delivery can be retried after an uncertain ACK,
    // but must preserve one child event and one durable notification.
    let _ = notices.fanout_chapter_releases(32).await.unwrap();
    let _ = notices.deliver_pending(64).await.unwrap();
    assert_eq!(release_children(), "1", "replay created another child event");
    assert_eq!(delivered_bells(), "1", "replay created another personal bell");
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM event_deliveries d
                 JOIN outbox_events e ON e.id=d.event_id
                 WHERE {release_child} AND d.consumer='user_notifications'"
            ),
        ),
        "1",
        "a single child event must have one durable delivery receipt"
    );
    let a_bells = notices
        .list_section(reader, None, 30, "releases")
        .await
        .unwrap();
    assert_eq!(
        a_bells
            .iter()
            .filter(|notice| notice.kind == "chapter_created")
            .count(),
        1,
        "A receives exactly one private bell for the real release"
    );
    let b_bells = notices
        .list_section(observer, None, 30, "releases")
        .await
        .unwrap();
    assert!(
        b_bells.is_empty(),
        "B has no Follow or Library subscription and must not get an E-08 bell"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events
                 WHERE event_type='reader.chapter.released' AND aggregate_id={released}
                   AND payload->>'recipient_user_id'='{observer}'"
            )
        ),
        "0",
        "fanout cannot create a private notification for the unfollowed observer"
    );
    // RET-02: another active reader may open the public book without Follow,
    // but does not inherit A's private release notification or read receipt.
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_book_follows
                 WHERE user_id={observer} AND book_id={book}"
            )
        ),
        "0"
    );
    let observer_updates = html(
        call(
            &app,
            "GET",
            "/me/library?view=updates",
            &observer_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(
        !observer_updates.contains("Demand fulfilled"),
        "reader B without Follow cannot see reader A's private update"
    );
    let observer_read = call(
        &app,
        "GET",
        &format!("/read/{released}"),
        &observer_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(
        observer_read.status(),
        StatusCode::OK,
        "a public FREE chapter does not require Follow"
    );
    assert!(html(observer_read).await.contains("C15 reader return"));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions
                 WHERE user_id={observer} AND chapter_id={released}"
            )
        ),
        "0",
        "opening without Follow is not a verified chapter completion"
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/me/updates/{released}/read-state"),
            &observer_cookie,
            ORIGIN,
            &format!("csrf_token={csrf}&state=read")
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "reader B cannot acknowledge A's private release notice"
    );
    // RET-00b/c: the same canonical first release must also advance the
    // library's single Continue Reading destination, without fabricating
    // a new saved position or treating the update as read.
    let returned_library = html(
        call(
            &app,
            "GET",
            "/me/library?view=reading",
            &reader_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(returned_library.contains(&format!("href=\"/read/{released}\"")));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT chapter_id FROM reader_book_positions WHERE user_id={reader} AND book_id={book}"
            )
        ),
        chapter.to_string(),
        "suggesting the next free chapter does not mutate durable progress"
    );
    // A delivered on-site release must support an actor-scoped read receipt.
    // A page view or successful chapter read does not silently acknowledge it.
    let read_state_path = format!("/me/updates/{released}/read-state");
    let mark_read = format!("csrf_token={csrf}&state=read");
    let mark_unread = format!("csrf_token={csrf}&state=unread");
    assert_eq!(
        call(
            &app,
            "POST",
            &read_state_path,
            &owner_cookie,
            ORIGIN,
            &mark_read
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "the author cannot acknowledge another reader's update"
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &read_state_path,
            &reader_cookie,
            "https://evil.invalid",
            &mark_read,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        assert_eq!(
            call(
                &app,
                "POST",
                &read_state_path,
                &reader_cookie,
                ORIGIN,
                &mark_read
            )
            .await
            .status(),
            StatusCode::SEE_OTHER,
            "acknowledging the same update is idempotent"
        );
    }
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_follow_notification_reads \
                 WHERE user_id={reader} AND chapter_id={released}"
            )
        ),
        "1"
    );
    let acknowledged = html(
        call(
            &app,
            "GET",
            "/me/library?view=updates",
            &reader_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(acknowledged.contains("Просмотрено"));
    assert!(acknowledged.contains("Отметить непрочитанной"));
    assert_eq!(
        call(
            &app,
            "POST",
            &read_state_path,
            &reader_cookie,
            ORIGIN,
            &mark_unread
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let unread = html(
        call(
            &app,
            "GET",
            "/me/library?view=updates",
            &reader_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(unread.contains("Отметить прочитанной"));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_follow_notification_reads \
                 WHERE user_id={reader} AND chapter_id={released}"
            )
        ),
        "0",
        "manual unread state cannot create a second release"
    );
    let returned_response = call(
        &app,
        "GET",
        &format!("/read/{released}"),
        &reader_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(returned_response.status(), StatusCode::OK);
    let returned = html(returned_response).await;
    assert!(returned.contains("C15 reader return"));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions \
                 WHERE user_id={reader} AND chapter_id={released}"
            )
        ),
        "0",
        "opening a new chapter is not itself a meaningful completion"
    );
    // The return is only meaningful after a version-validated end-of-text
    // completion. A page view cannot create a ChapterCompletedV1 fact.
    let released_version: i64 = pg(
        &url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={released}"),
    )
    .parse()
    .unwrap();
    let final_offset = "C15 reader return".encode_utf16().count();
    let finish_path = format!("/read/{released}/complete");
    let finish_form = format!(
        "csrf_token={csrf}&version_id={released_version}&block_index=1&char_offset={final_offset}&expected_revision=1"
    );
    assert_eq!(
        call(&app, "POST", &finish_path, "", ORIGIN, &finish_form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &finish_path,
            &reader_cookie,
            "https://evil.invalid",
            &finish_form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let unfinished = finish_form.replace(&format!("char_offset={final_offset}"), "char_offset=1");
    assert_eq!(
        call(
            &app,
            "POST",
            &finish_path,
            &reader_cookie,
            ORIGIN,
            &unfinished
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
        "middle-of-chapter progress must not count as completion"
    );
    let returned_completion = call(
        &app,
        "POST",
        &finish_path,
        &reader_cookie,
        ORIGIN,
        &finish_form,
    )
    .await;
    assert_eq!(returned_completion.status(), StatusCode::SEE_OTHER);
    let returned_location = format!("/read/{released}#continuation");
    assert_eq!(
        returned_completion
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some(returned_location.as_str())
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &finish_path,
            &reader_cookie,
            ORIGIN,
            &finish_form
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "replaying a stale completion revision cannot add another event"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions \
                 WHERE user_id={reader} AND chapter_id={released}"
            )
        ),
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events \
                 WHERE event_type='ChapterCompletedV1' \
                   AND dedupe_key='chapter-completed:{reader}:{released}'"
            )
        ),
        "1",
        "the returned reader's completion and outbox fact must be durable and unique"
    );
    let owner_fulfilled =
        html(call(&app, "GET", &owner_path, &owner_cookie, ORIGIN, "").await).await;
    assert!(owner_fulfilled
        .contains("Запросов после последней опубликованной главы: <strong>0</strong>"));

    // RET-02: Follow controls personal notifications, never the ability to
    // confirm a genuine completion of an otherwise public FREE chapter.
    let observer_finish = format!(
        "csrf_token={csrf}&version_id={released_version}&block_index=1&char_offset={final_offset}&expected_revision=0"
    );
    let wrong_version = observer_finish.replace(
        &format!("version_id={released_version}"),
        &format!("version_id={version}"),
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &finish_path,
            &observer_cookie,
            ORIGIN,
            &wrong_version
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "B cannot confirm a different immutable edition"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions
                 WHERE user_id={observer} AND chapter_id={released}"
            )
        ),
        "0"
    );
    // The second verified completion must be performed through Chromium's
    // real 390px mobile form, not synthesized by an HTTP test helper.
    // Desktop also proves that A returned to the proper new chapter.
    ret02_return_chromium::verify(
        &app,
        ORIGIN,
        &reader_session,
        &observer_session,
        &csrf,
        released,
        released_version,
    )
    .await;
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions
                 WHERE chapter_id={released} AND user_id IN ({reader},{observer})"
            )
        ),
        "2",
        "A and B have independent verified completion facts"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events
                 WHERE event_type='ChapterCompletedV1'
                   AND dedupe_key IN ('chapter-completed:{reader}:{released}',
                                      'chapter-completed:{observer}:{released}')"
            )
        ),
        "2",
        "completion outbox is recipient-specific, not a shared book event"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_follow_notification_reads
                 WHERE user_id={observer} AND chapter_id={released}"
            )
        ),
        "0",
        "B's actual reading must not forge A's Follow receipt"
    );

    // Completed-book label must not promise the next release or solicit demand.
    pg(
        &url,
        &format!("UPDATE books SET lifecycle_status='completed' WHERE id={book}"),
    );
    let finished_page = html(
        call(
            &app,
            "GET",
            &format!("/read/{released}"),
            &reader_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(finished_page.contains("Книга завершена"));
    assert!(!finished_page.contains("Хочу продолжение"));
    pg(
        &url,
        &format!("UPDATE books SET lifecycle_status='in_progress' WHERE id={book}"),
    );
    // The age gate is evaluated at *read and completion time*. Past progress
    // and Follow cannot act as a durable CanRead grant when the book changes
    // from basic to adult. Restore the fixture afterwards for paid regression.
    pg(
        &url,
        &format!("UPDATE books SET access_level='adult' WHERE id={book}"),
    );
    for cookie in [&reader_cookie, &observer_cookie] {
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/read/{released}"),
                cookie,
                ORIGIN,
                ""
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "a previous completion or Follow must not bypass current age policy"
        );
        assert_eq!(
            call(&app, "POST", &finish_path, cookie, ORIGIN, &observer_finish)
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "age denial must precede optimistic completion revision checks"
        );
    }
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions
                 WHERE chapter_id={released} AND user_id IN ({reader},{observer})"
            )
        ),
        "2",
        "age denial does not revoke or fabricate historical completion facts"
    );
    pg(
        &url,
        &format!("UPDATE books SET access_level='basic' WHERE id={book}"),
    );
    // A separate synthetic paid release remains as the old non-leak regression.

    let next: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},3,'Продолжение','public','paid',150) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let paid_version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({next},1,'PAID-CONTENT-SECRET') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({next},{paid_version})"
        ),
    );
    let after_release = html(call(&app, "GET", &read_path, &reader_cookie, ORIGIN, "").await).await;
    assert!(!after_release.contains("Хочу продолжение"));
    assert!(!after_release.contains("PAID-CONTENT-SECRET"));
    assert_eq!(
        call(&app, "POST", &want_path, &reader_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let owner_after = html(call(&app, "GET", &owner_path, &owner_cookie, ORIGIN, "").await).await;
    assert!(
        owner_after.contains("Запросов после последней опубликованной главы: <strong>0</strong>")
    );
    // The isolated tor_ci database is disposed after CI. Book ownership rows
    // deliberately have RESTRICT integrity; avoid false failures from a direct
    // book DELETE that bypasses the application ownership lifecycle.
    pool.close().await;
}
