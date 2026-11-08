//! D/H paged TOC HTTP checks, exercised within the existing real S034 purchase fixture.
use super::*;

pub(super) async fn check(
    browser: &Router,
    without_browser: &Router,
    chapter: i64,
    purchase: i64,
    buyer_cookie: &str,
    author_cookie: &str,
) {
    let route = format!(
        "/api/v1/reader/chapters/{chapter}/toc/window?purchase_id={purchase}&after=1.{chapter}",
    );
    assert_eq!(
        read_page(browser, &route, None).await.status(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        read_page(browser, &route, Some(author_cookie))
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        read_page(without_browser, &route, Some(buyer_cookie))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "the paid TOC route is not enabled on API-only routers",
    );
    let reply = read_page(browser, &route, Some(buyer_cookie)).await;
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
    let raw = to_bytes(reply.into_body(), 32_768).await.unwrap();
    assert!(!raw
        .windows(BODY.len())
        .any(|chunk| chunk == BODY.as_bytes()));
    assert!(!raw.windows(7).any(|chunk| chunk == b"dek_b64u"));
    let body: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        body["items"],
        json!([]),
        "one-chapter anchor has no successors"
    );
    assert_eq!(body["next_cursor"], Value::Null);
    for bad in [
        format!("/api/v1/reader/chapters/{chapter}/toc/window?purchase_id={purchase}"),
        format!("/api/v1/reader/chapters/{chapter}/toc/window?purchase_id={purchase}&after=0.0"),
        format!("/api/v1/reader/chapters/{chapter}/toc/window?purchase_id={purchase}&after=1.{chapter}&before=1.{chapter}"),
        format!("/api/v1/reader/chapters/{chapter}/toc/window?purchase_id={purchase}&after=1.{chapter}&limit=101"),
    ] {
        assert_eq!(
            read_page(browser, &bad, Some(buyer_cookie)).await.status(),
            StatusCode::BAD_REQUEST, "{bad}",
        );
    }
    let forged = format!(
        "/api/v1/reader/chapters/{chapter}/toc/window?purchase_id={purchase}&after=2.{chapter}",
    );
    assert_eq!(
        read_page(browser, &forged, Some(buyer_cookie))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "the cursor must refer to a chapter in its actual visible ordering",
    );
}
