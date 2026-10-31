//! Regression for the B-09 / D-05 raw DELETE membership fence.
use super::*;

fn assert_layer_delete_denied(error: sqlx::Error) {
    assert_eq!(
        error.as_database_error().and_then(|db| db.code()).as_deref(),
        Some("23514"),
        "unexpected database error: {error}"
    );
    assert!(
        error.to_string().contains("book membership deletion locked by Layer"),
        "DELETE should be blocked by the Layer-specific fence: {error}"
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_raw_owner_member_delete_denied_for_live_layer() {
    let (pool, repo, owner, buyer, book) = setup("member-delete-live").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 190, false).await;

    // The ordinary owner membership must not disappear via raw DELETE while
    // its Book is in the current Layer, even before any term is purchased.
    let error = sqlx::query(
        "DELETE FROM book_members WHERE book_id=$1 AND user_id=$2 AND role='owner'",
    )
    .bind(book)
    .bind(owner)
    .execute(&pool)
    .await
    .expect_err("active owner membership must be locked by Layer");
    assert_layer_delete_denied(error);
    let still_owner: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM book_members
             WHERE book_id=$1 AND user_id=$2 AND role='owner' AND is_active)",
    )
    .bind(book)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(still_owner);

    // Denied raw membership deletion cannot strand or alter paid entitlements.
    repo.buy(&cmd(buyer, layer, offer_id, version, 190, 'Q'))
        .await
        .unwrap();
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());

    // Historical inactive collaborators may still be cleaned up. Their
    // removal does not change owner identity or live Layer composition.
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',FALSE)",
    )
    .bind(book)
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let removed = sqlx::query(
        "DELETE FROM book_members WHERE book_id=$1 AND user_id=$2 AND NOT is_active",
    )
    .bind(book)
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(removed.rows_affected(), 1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_historical_paid_term_blocks_owner_member_delete_until_revoked() {
    let (pool, repo, owner, buyer, book) = setup("member-delete-historical").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 210, false).await;
    let paid = repo
        .buy(&cmd(buyer, layer, offer_id, version, 210, 'R'))
        .await
        .unwrap();
    repo.remove_book(id(owner), id(layer), id(book))
        .await
        .unwrap();

    // The Book is absent from the current Layer, but a paid term still
    // promises access to its historical composition.
    let currently_in_layer: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM commerce_layers l
             JOIN commerce_layer_version_books vb
               ON vb.layer_version_id=l.current_version_id
             WHERE vb.book_id=$1)",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!currently_in_layer);
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
    let error = sqlx::query(
        "DELETE FROM book_members WHERE book_id=$1 AND user_id=$2 AND role='owner'",
    )
    .bind(book)
    .bind(owner)
    .execute(&pool)
    .await
    .expect_err("historical paid Layer rights still bind the owner membership");
    assert_layer_delete_denied(error);

    // Once the last paid obligation is legally revoked and the Book is no
    // longer in any current Layer, the legacy B-09 cleanup path can proceed.
    // Roll back the probe so test fixtures retain canonical owner identity.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block','d05-member-delete-historical-revoke')",
    )
    .bind(paid.term_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let removed = sqlx::query(
        "DELETE FROM book_members WHERE book_id=$1 AND user_id=$2 AND role='owner'",
    )
    .bind(book)
    .bind(owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(removed.rows_affected(), 1);
    tx.rollback().await.unwrap();
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
    pool.close().await;
}
