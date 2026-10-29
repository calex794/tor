//! Authenticated browser render receipt is separate from credential issuance.
use super::*;

pub(super) async fn assert_render_receipt(
    app: &Router,
    browser: &Router,
    url: &str,
    ids: (i64, i64, i64),
    sessions: (&str, &str, &str, &str),
) -> (String, String, String) {
    let (chapter, version, purchase_id) = ids;
    let (buyer_cookie, csrf, author_cookie, author_csrf) = sessions;
    // A signed grant can be committed while its HTTP response gets lost.
    // It must NOT be silently called a completed browser opening.
    let open_route = format!("/api/v1/reader/chapters/{chapter}/open-confirmations");
    let open_body = json!({
        "purchase_id": purchase_id.to_string(), "version_id": version.to_string()
    })
    .to_string();
    assert_eq!(
        scalar(
            url,
            &format!(
        "SELECT count(*) FROM chapter_purchase_render_confirmations WHERE purchase_id={purchase_id}"
    )
        ),
        0,
        "issued grant != rendered chapter"
    );
    assert_eq!(
        send(
            app,
            &open_route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &open_body
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "API-only grant router must not mount browser ACK"
    );
    assert_eq!(
        send(browser, &open_route, None, None, Some(ORIGIN), &open_body)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            browser,
            &open_route,
            Some(buyer_cookie),
            Some(csrf),
            Some("https://hostile.invalid"),
            &open_body
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            browser,
            &open_route,
            Some(author_cookie),
            Some(author_csrf),
            Some(ORIGIN),
            &open_body
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            browser,
            &open_route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &json!({"purchase_id":purchase_id.to_string(),
                            "version_id":"99999999"})
            .to_string()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            browser,
            &open_route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &json!({"version_id":version.to_string(),
                            "source":"layer", "purchase_id":purchase_id.to_string()})
            .to_string()
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let opened = send(
        browser,
        &open_route,
        Some(buyer_cookie),
        Some(csrf),
        Some(ORIGIN),
        &open_body,
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    assert_eq!(opened.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(json_body(opened).await["status"], "recorded");
    let first_open = pg(url, &format!(
        "SELECT confirmed_at::TEXT FROM chapter_purchase_render_confirmations WHERE purchase_id={purchase_id}"
    ));
    assert_eq!(
        send(
            browser,
            &open_route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &open_body
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(pg(url, &format!(
        "SELECT confirmed_at::TEXT FROM chapter_purchase_render_confirmations WHERE purchase_id={purchase_id}"
    )), first_open, "render ACK replay does not alter first timestamp");
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM reader_wallet_entries WHERE purchase_id={purchase_id}")
        ),
        1,
        "render ACK cannot move money"
    );

    (open_route, open_body, first_open)
}
