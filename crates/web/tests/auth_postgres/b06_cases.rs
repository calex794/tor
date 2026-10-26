//! B-06: complete owner HTML journey on real isolated PostgreSQL.
use super::*;

pub(super) async fn publish_second_chapter(
    app: &Router,
    book_id: i64,
    book_path: &str,
    session: &str,
    author_cookies: &str,
) {
    let new_path = format!("/me/books/{book_id}/chapters/new");
    let page = send(app, request("GET", &new_path, "", author_cookies, ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let page_html = body_of(page).await;
    let csrf = csrf_token(&page_html);
    let command_key = page_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let post_path = format!("/me/books/{book_id}/chapters");
    let payload = format!(
        "number=2&title=%D0%92%D1%82%D0%BE%D1%80%D0%B0%D1%8F&body=PRIVATE%20B06%20V1&command_key={command_key}&csrf_token={csrf}"
    );
    assert_eq!(
        send(app, request("POST", &post_path, &payload, session, ORIGIN))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            app,
            request("POST", &post_path, &payload, author_cookies, "https://evil.example")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(app, request("POST", &post_path, &payload, author_cookies, ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(app, request("POST", &post_path, &payload, author_cookies, ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER,
        "same chapter create request must not duplicate chapters"
    );

    let public_page = send(
        app,
        request("GET", &format!("/books/{book_id}"), "", "", ORIGIN),
    )
    .await;
    assert_eq!(public_page.status(), StatusCode::OK);
    let public_html = body_of(public_page).await;
    assert!(!public_html.contains("PRIVATE B06"));
    assert!(!public_html.contains("Вторая"));
    let private_page = send(app, request("GET", book_path, "", author_cookies, ORIGIN)).await;
    assert_eq!(private_page.status(), StatusCode::OK);
    let private_html = body_of(private_page).await;
    assert!(private_html.contains("Черновик (не виден читателям)"));
    let edit_prefix = format!("href=\"/me/books/{book_id}/chapters/");
    let chapter_id = private_html
        .split(&edit_prefix)
        .nth(1)
        .expect("new chapter edit link")
        .split("/edit\"")
        .next()
        .unwrap();
    let edit_path = format!("/me/books/{book_id}/chapters/{chapter_id}/edit");
    let release_path = format!("/me/books/{book_id}/chapters/{chapter_id}/release");
    let first_release = send(
        app,
        request("GET", &release_path, "", author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(first_release.status(), StatusCode::OK);
    let first_release_html = body_of(first_release).await;
    assert!(first_release_html.contains("PRIVATE B06 V1"));
    let first_version = first_release_html
        .split("name=\"version_id\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let first_csrf = csrf_token(&first_release_html);
    let stale_release_payload =
        format!("version_id={first_version}&expected_revision=1&csrf_token={first_csrf}");
    let edit_page = send(app, request("GET", &edit_path, "", author_cookies, ORIGIN)).await;
    assert_eq!(edit_page.status(), StatusCode::OK);
    let edit_html = body_of(edit_page).await;
    let edit_csrf = csrf_token(&edit_html);
    let edit_key = edit_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let edit_payload = format!(
        "number=2&title=%D0%92%D1%82%D0%BE%D1%80%D0%B0%D1%8F&body=PUBLIC%20B06%20V2&expected_revision=1&command_key={edit_key}&csrf_token={edit_csrf}"
    );
    assert_eq!(
        send(
            app,
            request("POST", &edit_path, &edit_payload, author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(
            app,
            request("POST", &release_path, &stale_release_payload, author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "a previously opened publish form cannot release a stale revision"
    );

    let confirmed = send(app, request("GET", &release_path, "", author_cookies, ORIGIN)).await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmed_html = body_of(confirmed).await;
    assert!(confirmed_html.contains("PUBLIC B06 V2"));
    let version = confirmed_html
        .split("name=\"version_id\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let confirm_csrf = csrf_token(&confirmed_html);
    let release_payload =
        format!("version_id={version}&expected_revision=2&csrf_token={confirm_csrf}");
    assert_eq!(
        send(app, request("POST", &release_path, &release_payload, session, ORIGIN))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            app,
            request(
                "POST",
                &release_path,
                &release_payload,
                author_cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let published = send(
        app,
        request("POST", &release_path, &release_payload, author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(published.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        published.headers()[header::LOCATION],
        format!("/books/{book_id}")
    );
    assert_eq!(
        send(
            app,
            request("POST", &release_path, &release_payload, author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "same confirmation remains idempotent"
    );
    assert_eq!(
        send(app, request("GET", &edit_path, "", author_cookies, ORIGIN))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let public = send(
        app,
        request("GET", &format!("/books/{book_id}"), "", "", ORIGIN),
    )
    .await;
    assert_eq!(public.status(), StatusCode::OK);
    let html = body_of(public).await;
    assert!(html.contains("Вторая"));
    let chapter_link = format!("href=\"/reader/{chapter_id}\"");
    assert!(html.contains(&chapter_link));
    let reader = send(
        app,
        request("GET", &format!("/reader/{chapter_id}"), "", "", ORIGIN),
    )
    .await;
    assert_eq!(reader.status(), StatusCode::OK);
    assert!(body_of(reader).await.contains("PUBLIC B06 V2"));
}
