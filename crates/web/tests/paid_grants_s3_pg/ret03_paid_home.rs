//! RET-03c: paid-only Book Home shortcut verified through actual purchase + session.
//! This is owner-scoped navigation, never a substitute for a signed S3 grant.
use axum::{http::StatusCode, Router};

pub(super) async fn verify(
    app: &Router,
    url: &str,
    buyer: i64,
    book: i64,
    buyer_cookie: &str,
    stranger_cookie: &str,
) {
    assert_eq!(
        super::scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_library_items WHERE user_id={buyer} AND book_id={book}"
            ),
        ),
        0,
        "fixture must be an unbookmarked paid-only Book"
    );
    let response = super::private_get(app, "/me", Some(buyer_cookie)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = super::html_body(response).await;
    assert!(
        html.contains(&format!(
            "data-ret-purchased-book=\"{book}\" href=\"/me/library/{book}/saved\""
        )),
        "Home must link to the D-authorized private archive, not mint a paid grant"
    );
    assert!(!html.contains("purchase_id="));
    let stranger = super::private_get(app, "/me", Some(stranger_cookie)).await;
    assert_eq!(stranger.status(), StatusCode::OK);
    assert!(
        !super::html_body(stranger)
            .await
            .contains(&format!("data-ret-purchased-book=\"{book}\"")),
        "another reader cannot inherit a purchased-only Book shortcut"
    );
}
