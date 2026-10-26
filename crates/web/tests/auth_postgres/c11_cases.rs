//! C-11/S065: every free reading surface must enforce the same size boundary.
//! The only database used here is the isolated /tor_ci PostgreSQL fixture.
use super::*;

pub(super) async fn oversized_chapter_is_denied_across_reader_and_progress(
    app: &Router,
    session: &str,
    csrf_cookies: &str,
    csrf: &str,
) {
    // The isolated /tor_ci workflow seeds a published legacy chapter exceeding
    // the 50,000-Unicode-character reader limit. No DB writes belong in web tests.
    // A public chapter still works for guests and authenticated readers.
    assert_eq!(
        send(app, request("GET", "/reader/101", "", "", ORIGIN))
            .await
            .status(),
        StatusCode::OK,
    );
    assert_eq!(
        send(
            app,
            request(
                "GET",
                "/api/v1/me/progress/free?chapter_id=101",
                "",
                session,
                ORIGIN
            ),
        )
        .await
        .status(),
        StatusCode::OK,
    );

    // No alternate read or progress surface may bypass the reader's 50k cap.
    for (path, cookies) in [
        ("/reader/98111", ""),
        ("/read/98111", session),
        ("/api/v1/me/progress/free?chapter_id=98111", session),
    ] {
        let reply = send(app, request("GET", path, "", cookies, ORIGIN)).await;
        assert_eq!(reply.status(), StatusCode::PAYLOAD_TOO_LARGE, "{path}");
        assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store", "{path}");
        let text = body_of(reply).await;
        assert!(
            text.is_empty(),
            "reader errors must not contain chapter text: {path}"
        );
    }

    let json = format!(
        "{{\"chapter_id\":\"{CHAPTER_ID}\",\"version_id\":\"{VERSION_ID}\",\"block_id\":1,\"char_offset\":0,\"expected_revision\":0}}"
    );
    let reply = send(
        app,
        json_request(
            "PUT",
            "/api/v1/me/progress/free",
            &json,
            csrf_cookies,
            ORIGIN,
            csrf,
        ),
    )
    .await;
    assert_eq!(reply.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_of(reply).await.is_empty());

    let form = format!("csrf_token={csrf}&version_id={VERSION_ID}&block_index=1&char_offset=0");
    let reply = send(
        app,
        request("POST", "/read/98111/place", &form, csrf_cookies, ORIGIN),
    )
    .await;
    assert_eq!(reply.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_of(reply).await.is_empty());

    // Query parameters never turn a disabled direct raw reader into a shortcut.
    let reply = send(
        app,
        request(
            "GET",
            "/api/v1/chapters/98111/read?preview=1&purchase_id=1",
            "",
            session,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(reply.status(), StatusCode::NOT_FOUND);
    assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_of(reply).await.is_empty());

}
