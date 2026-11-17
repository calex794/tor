//! D-06: a committed first protected grant is not proof of client delivery.
//! This helper models a lost HTTP response after Axum has finished handling
//! the request. It does not claim to simulate a real socket disconnect or S3 GET.
use super::*;

fn grant_evidence_sql(purchase_id: i64, buyer: i64) -> String {
    format!(
        "SELECT object_kind || '|' || granted_at::text \
         FROM chapter_purchase_first_exposures \
         WHERE purchase_id={purchase_id} AND user_id={buyer}"
    )
}

fn financial_snapshot_sql(purchase_id: i64, buyer: i64) -> String {
    format!(
        "SELECT balance_minor::text || '|' || \
             (SELECT COUNT(*) FROM reader_wallet_entries \
              WHERE purchase_id={purchase_id})::text || '|' || \
             (SELECT COUNT(*) FROM library_purchased_chapters \
              WHERE purchase_id={purchase_id})::text \
         FROM reader_wallet_accounts WHERE user_id={buyer}"
    )
}

/// The handler returns 200, but the simulated client discards the complete
/// response without decoding the key or performing the signed S3 GET.
/// The database evidence still records issuance before client consumption.
pub(super) async fn discard_first_response(
    app: &Router,
    url: &str,
    buyer: i64,
    chapter: i64,
    purchase_id: i64,
    buyer_cookie: &str,
    csrf: &str,
) -> (String, String) {
    let evidence_sql = grant_evidence_sql(purchase_id, buyer);
    let money_sql = financial_snapshot_sql(purchase_id, buyer);
    assert_eq!(
        pg(url, &evidence_sql),
        "",
        "checkout must not mark the purchased chapter as opened"
    );
    let before = pg(url, &money_sql);

    let response = post_grant(app, chapter, Some(buyer_cookie), Some(csrf), ORIGIN, "{}").await;
    assert_eq!(response.status(), StatusCode::OK);
    drop(response); // No client JSON parse, key use, ciphertext GET or decryption.

    let marker = pg(url, &evidence_sql);
    assert!(
        marker.starts_with("chapter|"),
        "issuer recorded a grant even though the response was discarded"
    );
    assert_eq!(pg(url, &money_sql), before, "grant must not move money");
    (marker, before)
}

/// A later successful retry delivers the same pinned edition without
/// overwriting first issuance or performing another charge.
pub(super) fn assert_retry_preserves_evidence(
    url: &str,
    buyer: i64,
    purchase_id: i64,
    original: &(String, String),
) {
    assert_eq!(
        pg(url, &grant_evidence_sql(purchase_id, buyer)),
        original.0,
        "first grant timestamp and object kind are immutable"
    );
    assert_eq!(
        pg(url, &financial_snapshot_sql(purchase_id, buyer)),
        original.1,
        "retry must not duplicate purchase, entitlement or wallet entry"
    );
}

pub(super) async fn assert_purchase_list_bound_to_buyer(
    app: &Router,
    chapter: i64,
    purchase_id: i64,
    buyer_cookie: &str,
    stranger_cookie: &str,
    csrf: &str,
) {
    let stranger_list = private_get(app, "/me/purchases", Some(stranger_cookie)).await;
    assert_eq!(stranger_list.status(), StatusCode::OK);
    assert!(
        !html_body(stranger_list)
            .await
            .contains(&format!("purchase_id={purchase_id}")),
        "another account cannot enumerate purchases"
    );
    let owned_list = private_get(app, "/me/purchases", Some(buyer_cookie)).await;
    assert_eq!(owned_list.status(), StatusCode::OK);
    assert_eq!(owned_list.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(owned_list.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(owned_list.headers()["x-robots-tag"], "noindex");
    assert!(html_body(owned_list)
        .await
        .contains(&format!("/read/paid/{chapter}?purchase_id={purchase_id}")));
    let ordinary_library = private_get(app, "/me/library", Some(buyer_cookie)).await;
    assert_eq!(ordinary_library.status(), StatusCode::OK);
    assert!(html_body(ordinary_library)
        .await
        .contains("href=\"/me/purchases\""));
    assert_eq!(
        post_grant(
            app,
            chapter,
            Some(stranger_cookie),
            Some(csrf),
            ORIGIN,
            &json!({"purchase_id":purchase_id.to_string()}).to_string(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "another actor cannot reuse the buyer's purchase ID"
    );
}
