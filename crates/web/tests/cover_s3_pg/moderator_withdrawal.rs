//! Full S3-backed moderator cover withdrawal acceptance on disposable tor_ci.
use super::*;
use tor_books::repositories::cover_slots::PublicCoverObject;

pub(super) async fn verify(
    slots: &PgCoverSlots,
    reviews: &PgCoverReview,
    ledger: &PgPublicCoverPublications,
    store: &S3CoverStore,
    card: &PublicCoverObject,
    session_digest: &[u8; 32],
    app: &axum::Router,
    job: i64,
) {
    // A withdrawn approved variant cannot be used by same-origin or CDN,
    // but the immutable private original remains for audit.
    reviews
        .withdraw_current(session_digest, BOOK_ID, 1, "Policy withdrawal")
        .await
        .unwrap();
    assert_eq!(slots.current_slot(BOOK_ID).await.unwrap(), Some((job, 2)));
    assert!(slots
        .public_variant(BOOK_ID, PublicCoverKind::Card)
        .await
        .unwrap()
        .is_none());
    assert!(slots
        .public_card_ids(&[BOOK_ID as u64])
        .await
        .unwrap()
        .is_empty());
    assert!(ledger
        .current_cdn_copies(&[BOOK_ID], PublicCoverKind::Main)
        .await
        .unwrap()
        .is_empty());
    assert!(ledger.publication_candidates(3).await.unwrap().is_empty());
    assert_eq!(
        request(app, "/covers/950/card.webp").await.status(),
        StatusCode::NOT_FOUND
    );
    assert!(
        store
            .exists_verified(&card.storage_key, &card.sha256, card.size_bytes)
            .await
            .unwrap(),
        "moderator withdrawal must retain private verified bytes"
    );
    assert!(reviews
        .private_history(session_digest, BOOK_ID)
        .await
        .unwrap()
        .iter()
        .any(|row| row.outcome == "withdrawn"));
}
