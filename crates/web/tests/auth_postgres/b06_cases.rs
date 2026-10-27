//! H BKR-96: adding a new free chapter to a published book is one Save.
//! B-06's explicit release route remains for existing historical drafts.
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
    assert!(page_html.contains("сразу после сохранения"));
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
        "number=2&title=%D0%92%D1%82%D0%BE%D1%80%D0%B0%D1%8F&body=PUBLIC%20B06%20V1&command_key={command_key}&csrf_token={csrf}"
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
            request(
                "POST",
                &post_path,
                &payload,
                author_cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let saved = send(
        app,
        request("POST", &post_path, &payload, author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        send(
            app,
            request("POST", &post_path, &payload, author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "a retried Save must not add a second public version"
    );
    let altered = payload.replace("PUBLIC%20B06%20V1", "CHANGED%20B06");
    assert_eq!(
        send(
            app,
            request("POST", &post_path, &altered, author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let public = send(
        app,
        request("GET", &format!("/books/{book_id}"), "", "", ORIGIN),
    )
    .await;
    assert_eq!(public.status(), StatusCode::OK);
    let html = body_of(public).await;
    assert!(html.contains("Вторая"), "one Save must show the new free chapter");
    let chapter_id = html
        .split("href=\"/reader/")
        .last()
        .expect("new public chapter reader link")
        .split('"')
        .next()
        .unwrap();
    assert!(!chapter_id.is_empty());
    let reader = send(
        app,
        request("GET", &format!("/reader/{chapter_id}"), "", "", ORIGIN),
    )
    .await;
    assert_eq!(reader.status(), StatusCode::OK);
    assert!(body_of(reader).await.contains("PUBLIC B06 V1"));

    let private = send(app, request("GET", book_path, "", author_cookies, ORIGIN)).await;
    assert_eq!(private.status(), StatusCode::OK);
    let private_html = body_of(private).await;
    assert!(!private_html.contains("Черновик (не виден читателям)"));
    // No second release page exists for an already published free chapter.
    for suffix in ["edit", "release"] {
        assert_eq!(
            send(
                app,
                request(
                    "GET",
                    &format!("/me/books/{book_id}/chapters/{chapter_id}/{suffix}"),
                    "",
                    author_cookies,
                    ORIGIN,
                ),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
}
