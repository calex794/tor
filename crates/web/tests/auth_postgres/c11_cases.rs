//! C-11/S065: one access policy for real public and private reading routes.
//! These checks use only existing disposable PostgreSQL fixtures (101..108).
use super::*;
use tor_reading::domain::renderer::valid_reader_length;

pub(super) async fn reader_and_progress_share_access_policy(
    app: &Router,
    session: &str,
    csrf_cookies: &str,
    csrf: &str,
) {
    // Unicode length is not UTF-8 byte length. Production PostgreSQL forbids
    // values above the limit; the renderer and progress routes reject them too.
    assert!(valid_reader_length(&"😀".repeat(50_000)));
    assert!(!valid_reader_length(&"😀".repeat(50_001)));

    for (path, cookies) in [
        ("/reader/101", ""),
        ("/read/101", session),
        ("/api/v1/me/progress/free?chapter_id=101", session),
    ] {
        let reply = send(app, request("GET", path, "", cookies, ORIGIN)).await;
        assert_eq!(reply.status(), StatusCode::OK, "{path}");
        assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store", "{path}");
    }
    let guest_progress = send(
        app,
        request(
            "GET",
            "/api/v1/me/progress/free?chapter_id=101",
            "",
            "",
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(guest_progress.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(guest_progress.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_of(guest_progress).await.is_empty());

    // Published paid, draft, hidden book, held, scheduled and moderated versions.
    // Every route denies them even when the caller is logged in and supplies a
    // matching CSRF token and an invented version number.
    for (chapter, version) in [
        (102, 202),
        (103, 203),
        (104, 204),
        (105, 205),
        (106, 206),
        (107, 207),
    ] {
        let public = format!("/reader/{chapter}");
        let private = format!("/read/{chapter}");
        let progress = format!("/api/v1/me/progress/free?chapter_id={chapter}");
        for (path, cookies) in [
            (public.as_str(), ""),
            (private.as_str(), session),
            (progress.as_str(), session),
        ] {
            let reply = send(app, request("GET", path, "", cookies, ORIGIN)).await;
            assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store", "{path}");
            assert!(
                body_of(reply).await.is_empty(),
                "denied read must not include hidden chapter bytes: {path}"
            );
        }

        let save = format!(
            "{{\"chapter_id\":\"{chapter}\",\"version_id\":\"{version}\",\"block_id\":1,\"char_offset\":0,\"expected_revision\":0}}"
        );
        let reply = send(
            app,
            json_request(
                "PUT",
                "/api/v1/me/progress/free",
                &save,
                csrf_cookies,
                ORIGIN,
                csrf,
            ),
        )
        .await;
        assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{chapter}");
        assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
        assert!(body_of(reply).await.is_empty());

        let place = format!("csrf_token={csrf}&version_id={version}&block_index=1&char_offset=0");
        let reply = send(
            app,
            request(
                "POST",
                &format!("/read/{chapter}/place"),
                &place,
                csrf_cookies,
                ORIGIN,
            ),
        )
        .await;
        assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{chapter}");
        assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
        assert!(body_of(reply).await.is_empty());

        let completed = format!("{place}&expected_revision=0");
        let reply = send(
            app,
            request(
                "POST",
                &format!("/read/{chapter}/complete"),
                &completed,
                csrf_cookies,
                ORIGIN,
            ),
        )
        .await;
        assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{chapter}");
        assert!(
            body_of(reply).await.is_empty(),
            "completion leaked hidden chapter"
        );
    }

    // A bookmark, forged purchase ID or preview query grants no reading rights.
    for path in [
        "/api/v1/chapters/101/read?preview=1",
        "/api/v1/chapters/103/read?purchase_id=999&preview=1",
        "/api/v1/chapters/102/read?version_id=202",
    ] {
        let reply = send(app, request("GET", path, "", session, ORIGIN)).await;
        assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
        assert!(body_of(reply).await.is_empty());
    }
    let forged = send(
        app,
        request(
            "GET",
            "/api/v1/me/progress/purchased?purchase_id=999&chapter_id=103&version_id=203",
            "",
            session,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(forged.status(), StatusCode::NOT_FOUND);
    assert_eq!(forged.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_of(forged).await.is_empty());
}

pub(super) async fn reopened_reader_has_saved_anchor(app: &Router, session: &str) {
    let resumed = send(app, request("GET", "/read/101", "", session, ORIGIN)).await;
    assert_eq!(resumed.status(), StatusCode::OK);
    assert_eq!(resumed.headers()[header::CACHE_CONTROL], "no-store");
    let html = body_of(resumed).await;
    assert!(html.contains("href=\"#p-1\""));
    assert!(html.contains("id=\"p-1\""));
    assert!(!html.contains("Место ещё не сохранено"));
}

pub(super) async fn resume_after_login(app: &Router, logged: &Response) {
    let renewed_session = cookie(logged, "tor-dev-session=");
    let library = send(
        app,
        request("GET", "/me/library", "", &renewed_session, ORIGIN),
    )
    .await;
    assert_eq!(library.status(), StatusCode::OK);
    assert_eq!(library.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_of(library).await.contains("href=\"/read/101#p-1\""));
    reopened_reader_has_saved_anchor(app, &renewed_session).await;
}

pub(super) async fn second_session_reads_saved_progress(app: &Router, session: &str) {
    let reply = send(
        app,
        json_request(
            "GET",
            "/api/v1/me/progress/free?chapter_id=101",
            "",
            session,
            ORIGIN,
            "",
        ),
    )
    .await;
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
    let json = body_of(reply).await;
    assert!(json.contains(r#""revision":2"#));
    assert!(json.contains(r#""char_offset":1"#));
}

pub(super) async fn stale_second_session_cannot_overwrite(app: &Router, session: &str) {
    let page = send(app, request("GET", "/read/101", "", session, ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let token = csrf_token(&body_of(page).await).to_owned();
    let cookies = format!("{session}; {csrf_cookie}");
    let stale = r#"{"chapter_id":"101","version_id":"201","block_id":1,"char_offset":0,"expected_revision":1}"#;
    let reply = send(
        app,
        json_request(
            "PUT",
            "/api/v1/me/progress/free",
            stale,
            &cookies,
            ORIGIN,
            &token,
        ),
    )
    .await;
    assert_eq!(reply.status(), StatusCode::CONFLICT);
    second_session_reads_saved_progress(app, session).await;
}

pub(super) async fn concurrent_sessions_one_progress_winner(app: &Router, session: &str) {
    let page = send(app, request("GET", "/read/101", "", session, ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let token = csrf_token(&body_of(page).await).to_owned();
    let cookies = format!("{session}; {csrf_cookie}");
    let left = r#"{"chapter_id":"101","version_id":"201","block_id":1,"char_offset":2,"expected_revision":2}"#;
    let right = r#"{"chapter_id":"101","version_id":"201","block_id":1,"char_offset":3,"expected_revision":2}"#;
    let left_request = json_request(
        "PUT",
        "/api/v1/me/progress/free",
        left,
        &cookies,
        ORIGIN,
        &token,
    );
    let right_request = json_request(
        "PUT",
        "/api/v1/me/progress/free",
        right,
        &cookies,
        ORIGIN,
        &token,
    );
    let (a, b) = tokio::join!(send(app, left_request), send(app, right_request));
    let statuses = [a.status(), b.status()];
    assert_eq!(statuses.iter().filter(|&&s| s == StatusCode::OK).count(), 1);
    assert_eq!(
        statuses
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    let state = send(
        app,
        json_request(
            "GET",
            "/api/v1/me/progress/free?chapter_id=101",
            "",
            session,
            ORIGIN,
            "",
        ),
    )
    .await;
    assert_eq!(state.status(), StatusCode::OK);
    let body = body_of(state).await;
    assert!(body.contains(r#""revision":3"#));
    assert!(body.contains(r#""char_offset":2"#) || body.contains(r#""char_offset":3"#));
}

/// Browser-facing C-05: a re-published chapter must discard stale V1 anchors,
/// while still letting the reader save a new position through the HTML form.
pub(super) async fn reader_v2_html_flow(app: &Router, session: &str, chapter_raw: &str) {
    let chapter_id = chapter_raw.parse::<i64>().expect("published chapter ID");
    let path = format!("/read/{chapter_id}");
    let save_path = format!("{path}/place");
    let page = send(app, request("GET", &path, "", session, ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let html = body_of(page).await;
    assert!(html.contains("Место ещё не сохранено"));
    assert!(!html.contains("Глава обновлена. Сохранённое место"));
    let old_version = html
        .split("name=\"version_id\" value=\"")
        .nth(1)
        .expect("published version on reader page")
        .split('"')
        .next()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let csrf = csrf_token(&html);
    let cookies = format!("{session}; {csrf_cookie}");
    let saved_form =
        format!("csrf_token={csrf}&version_id={old_version}&block_index=1&char_offset=0");
    let saved = send(
        app,
        request("POST", &save_path, &saved_form, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let library = send(app, request("GET", "/me/library", "", session, ORIGIN)).await;
    let library_html = body_of(library).await;
    assert!(library_html.contains(&format!("href=\"{path}#p-1\"")));

    // The Web crate deliberately has no SQLx dependency. Use the CI-installed
    // PostgreSQL client to simulate publication in the isolated test database.
    let url = std::env::var("TEST_DATABASE_URL").expect("test-only PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never write to production");
    let sql = format!(
        "WITH added AS ( \
          INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
          SELECT {chapter_id}, COALESCE(MAX(version_no),0)+1, 'V2-UPDATED-TEXT' \
          FROM chapter_versions WHERE chapter_id={chapter_id} RETURNING id \
         ) UPDATE chapter_publications SET version_id=added.id \
         FROM added WHERE chapter_publications.chapter_id={chapter_id} \
         RETURNING added.id"
    );
    let output = std::process::Command::new("psql")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1"])
        .args(["-d", &url, "-c", &sql])
        .output()
        .expect("psql installed in PostgreSQL integration workflow");
    assert!(
        output.status.success(),
        "test publication failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let newer = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse::<i64>()
        .expect("new publication version ID");
    let changed_library = send(app, request("GET", "/me/library", "", session, ORIGIN)).await;
    let changed_library_html = body_of(changed_library).await;
    assert!(changed_library_html.contains("Глава обновлена"));
    assert!(changed_library_html.contains(&format!("href=\"{path}\"")));
    assert!(!changed_library_html.contains(&format!("href=\"{path}#p-1\"")));

    let changed = send(app, request("GET", &path, "", session, ORIGIN)).await;
    assert_eq!(changed.status(), StatusCode::OK);
    let new_csrf_cookie = cookie(&changed, "tor-dev-csrf=");
    let changed_html = body_of(changed).await;
    assert!(changed_html.contains("V2-UPDATED-TEXT"));
    assert!(!changed_html.contains("Перейти к сохранённому месту"));
    assert!(changed_html.contains("Глава обновлена. Сохранённое место"));
    assert!(!changed_html.contains("Место ещё не сохранено"));
    assert!(changed_html.contains(&format!("name=\"version_id\" value=\"{newer}\"")));
    let new_csrf = csrf_token(&changed_html);
    let new_cookies = format!("{session}; {new_csrf_cookie}");

    let stale_form =
        format!("csrf_token={new_csrf}&version_id={old_version}&block_index=1&char_offset=0");
    let stale = send(
        app,
        request("POST", &save_path, &stale_form, &new_cookies, ORIGIN),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let new_form = format!("csrf_token={new_csrf}&version_id={newer}&block_index=1&char_offset=0");
    let saved_new = send(
        app,
        request("POST", &save_path, &new_form, &new_cookies, ORIGIN),
    )
    .await;
    assert_eq!(saved_new.status(), StatusCode::SEE_OTHER);
    assert_eq!(saved_new.headers()[header::LOCATION], format!("{path}#p-1"));
    let resumed = send(app, request("GET", &path, "", session, ORIGIN)).await;
    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed_html = body_of(resumed).await;
    assert!(resumed_html.contains("href=\"#p-1\""));
    assert!(!resumed_html.contains("Глава обновлена. Сохранённое место"));
}
