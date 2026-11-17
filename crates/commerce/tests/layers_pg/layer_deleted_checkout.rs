//! D-05: a new Layer checkout must not sell a removed Book or allocate
//! earnings to it. Historical paid composition versions remain immutable.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_new_layer_sale_waits_for_clean_offer_after_ordinary_deletion() {
    let (pool, layers, owner, buyer, book) = setup("deleted_offer_guard").await;
    let (layer, first_version, offer_id) = offer(&layers, owner, book, 200, false).await;
    let sibling: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES('ci:d05:sibling:deleted_offer_guard','ru','Live sibling','public',$1,'basic')
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version_with_both = layers
        .add_book(id(owner), id(layer), id(sibling))
        .await
        .unwrap();
    assert_ne!(first_version, version_with_both);
    let original = layers
        .buy(&cmd(buyer, layer, offer_id, version_with_both, 200, 'A'))
        .await
        .unwrap();
    assert!(!original.replayed);

    // Simulate B's completed *ordinary* deletion. The S034 full-stack
    // integration separately exercises owner step-up + moderator approval.
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
                ordinary_deleted_at=clock_timestamp(),visibility='draft'
         WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    let late: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('d05-late-deleted-offer@example.test','Late buyer','$argon2id$ci')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1500,'RUB')",
    )
    .bind(late)
    .execute(&pool)
    .await
    .unwrap();

    // The old Layer version contains two Books but only the sibling can
    // legally be sold to new buyers. Reject the stale quote before wallet
    // movements instead of allocating proceeds to deleted content.
    assert_eq!(
        layers
            .buy(&cmd(late, layer, offer_id, version_with_both, 200, 'B'))
            .await,
        Err(LayerError::Inaccessible)
    );
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(late)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(balance, 1500);
    let late_finance: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reader_wallet_entries WHERE user_id=$1")
            .bind(late)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(late_finance, 0);

    // Owner removes the Book from CURRENT only; old paid versions and
    // original customer's immutable purchase continue to exist unchanged.
    let clean = layers
        .remove_book(id(owner), id(layer), id(book))
        .await
        .unwrap();
    assert_ne!(clean, version_with_both);
    let old_membership: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM commerce_layer_version_books
         WHERE layer_version_id=$1 AND book_id=$2",
    )
    .bind(version_with_both)
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(old_membership, 1);
    let clean_membership: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM commerce_layer_version_books
         WHERE layer_version_id=$1 AND book_id=$2",
    )
    .bind(clean)
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(clean_membership, 0);

    let paid = layers
        .buy(&cmd(late, layer, offer_id, clean, 200, 'C'))
        .await
        .unwrap();
    assert!(!paid.replayed);
    assert_eq!(paid.balance_after_minor, 1300);
    let snapshots: Vec<i64> = sqlx::query_scalar(
        "SELECT book_id FROM commerce_layer_term_share_snapshots
         WHERE term_id=$1 ORDER BY book_id",
    )
    .bind(paid.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(snapshots, vec![sibling]);

    // The same old buyer may purchase another 30-day period BEFORE the
    // current one expires. Their next term inherits the original origin and
    // therefore pays for/retains the archived Book while a new buyer never
    // acquires it. Rejecting every historical Book in origin..base would
    // silently destroy this intentional cumulative-rights contract.
    let continued = layers
        .buy(&cmd(buyer, layer, offer_id, clean, 200, 'D'))
        .await
        .unwrap();
    assert!(!continued.replayed);
    let inherited: bool = sqlx::query_scalar(
        "SELECT renewal.access_origin_version_id=original.access_origin_version_id
         FROM commerce_layer_terms renewal
         JOIN commerce_layer_terms original ON original.id=$2
         WHERE renewal.id=$1",
    )
    .bind(continued.term_id)
    .bind(original.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(inherited, "continuous prepaid renewal retains the original Layer origin");
    let mut old_renewed_books: Vec<i64> = sqlx::query_scalar(
        "SELECT book_id FROM commerce_layer_term_share_snapshots
         WHERE term_id=$1 ORDER BY book_id",
    )
    .bind(continued.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    old_renewed_books.sort();
    let mut inherited_expected = vec![book, sibling];
    inherited_expected.sort();
    assert_eq!(
        old_renewed_books, inherited_expected,
        "renewing an old term must retain archived Book economics"
    );

    let original_members: Vec<i64> = sqlx::query_scalar(
        "SELECT book_id FROM commerce_layer_term_share_snapshots
         WHERE term_id=$1 ORDER BY book_id",
    )
    .bind(original.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut expected = vec![book, sibling];
    expected.sort();
    assert_eq!(original_members, expected, "old paid contract is immutable");
}
