//! Isolated B-04 author browser scenario, split to keep each test file bounded.
use super::*;

pub(super) async fn draft_edit_case(
    app: &Router,
    book_id: i64,
    book_path: &str,
    session: &str,
    author_cookies: &str,
    author_book_html: &str,
) -> String {
    // B-04: browser edit reads the stored working text and expected revision.
    let edit_prefix = format!("href=\"/me/books/{book_id}/chapters/");
    let chapter_id = author_book_html
        .split(&edit_prefix)
        .nth(1)
        .expect("draft edit link")
        .split("/edit\"")
        .next()
        .expect("chapter ID");
    let edit_path = format!("/me/books/{book_id}/chapters/{chapter_id}/edit");
    assert_eq!(
        send(&app, request("GET", &edit_path, "", "", ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let editor = send(
        &app,
        request("GET", &edit_path, "", &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(editor.status(), StatusCode::OK);
    assert_eq!(editor.headers()[header::CACHE_CONTROL], "no-store");
    let edit_html = body_of(editor).await;
    assert!(edit_html.contains("name=\"expected_revision\" value=\"1\""));
    assert!(edit_html.contains("Real chapter"));
    let edit_key = edit_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .expect("edit key")
        .split('"')
        .next()
        .unwrap();
    let edit_csrf = csrf_token(&edit_html);
    let edit_payload = format!(
        "number=1&title=%D0%93%D0%BB%D0%B0%D0%B2%D0%B0&body=%23%20Updated%20chapter&expected_revision=1&command_key={edit_key}&csrf_token={edit_csrf}"
    );
    assert_eq!(
        send(
            &app,
            request("POST", &edit_path, &edit_payload, &session, ORIGIN)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                &edit_path,
                &edit_payload,
                &author_cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let updated = send(
        &app,
        request("POST", &edit_path, &edit_payload, &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::SEE_OTHER);
    assert_eq!(updated.headers()[header::LOCATION].to_str().unwrap(), book_path);
    assert_eq!(
        send(
            &app,
            request("POST", &edit_path, &edit_payload, &author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "repeat of a successful command must be harmless"
    );
    let stale_payload = edit_payload.replace(edit_key, &"s".repeat(43));
    assert_eq!(
        send(
            &app,
            request("POST", &edit_path, &stale_payload, &author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "stale tab must not overwrite revision two"
    );
    let editor_again = send(
        &app,
        request("GET", &edit_path, "", &author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(editor_again.status(), StatusCode::OK);
    let updated_html = body_of(editor_again).await;
    assert!(updated_html.contains("name=\"expected_revision\" value=\"2\""));
    assert!(updated_html.contains("Updated chapter"));
    edit_path
}
