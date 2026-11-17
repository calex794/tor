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
