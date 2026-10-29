//! S031: public Layer price and composition are a read-only, versioned quote.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_layer_offer_quote_is_complete_and_never_authoritative_for_checkout() {
    let (pool, repo, owner, buyer, book) = setup("quote-complete").await;
    let layer = repo.create_layer(id(owner), "Layer <test>").await.unwrap();
    assert_eq!(
        repo.offer_quote(id(layer)).await,
        Err(LayerError::Inaccessible),
        "never advertise a Layer without a published offer"
    );
    let version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let offer = repo
        .publish_offer(id(owner), id(layer), 251, false)
        .await
        .unwrap();
    let first = repo.offer_quote(id(layer)).await.unwrap();
    assert_eq!(first.layer_id, layer);
    assert_eq!(first.title, "Layer <test>");
    assert_eq!(first.offer_id, offer);
    assert_eq!(first.version_id, version);
    assert_eq!(first.price_minor, 251);
    assert_eq!(first.term_kind, tor_commerce::domain::layers::LayerTermKind::Prepaid30Days);
    assert_eq!(first.books.len(), 1);
    assert_eq!(first.books[0].book_id, book);

    let second_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Second of two','public',$2) RETURNING id",
    )
    .bind(format!("ci:d05:quote-second:{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let revision = repo
        .add_book(id(owner), id(layer), id(second_book))
        .await
        .unwrap();
    let changed = repo.offer_quote(id(layer)).await.unwrap();
    assert_eq!(changed.version_id, revision);
    assert_eq!(changed.offer_id, offer, "offer ID does not silently change");
    assert_eq!(changed.books.len(), 2);
    assert_eq!(
        changed.books.iter().map(|b| b.book_id).collect::<Vec<_>>(),
        vec![book, second_book],
    );
    // A sale requires the latest quote; a quote never grants reading or money.
    let purchases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_terms WHERE layer_id=$1",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purchases, 0);
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.offer_quote(id(layer)).await,
        Err(LayerError::Inaccessible),
        "an incomplete or held composition must not be advertised"
    );
    assert_eq!(
        repo.buy(&cmd(buyer, layer, offer, revision, 251, 'H')).await,
        Err(LayerError::Inaccessible),
        "the wallet may not be debited for a held Layer Book"
    );
    let balance: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1",
    )
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(balance, 1500);
    sqlx::query("UPDATE books SET admin_hold=FALSE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE commerce_layers SET status='blocked' WHERE id=$1")
        .bind(layer)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.offer_quote(id(layer)).await,
        Err(LayerError::Inaccessible)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_old_held_book_does_not_block_fresh_buyer_of_current_layer() {
    let (pool, repo, owner, buyer, previous) = setup("quote-old-held").await;
    let layer = repo.create_layer(id(owner), "Current book only").await.unwrap();
    repo.add_book(id(owner), id(layer), id(previous))
        .await
        .unwrap();
    let offer = repo.publish_offer(id(owner), id(layer), 251, false).await.unwrap();
    let current_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Current valid','public',$2) RETURNING id",
    )
    .bind(format!("ci:d05:quote-current:{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    repo.add_book(id(owner), id(layer), id(current_book))
        .await
        .unwrap();
    let current = repo
        .remove_book(id(owner), id(layer), id(previous))
        .await
        .unwrap();
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(previous)
        .execute(&pool)
        .await
        .unwrap();
    let quote = repo.offer_quote(id(layer)).await.unwrap();
    assert_eq!(quote.books.len(), 1);
    assert_eq!(quote.books[0].book_id, current_book);
    assert_eq!(quote.version_id, current);

    let paid = repo.buy(&cmd(buyer, layer, offer, current, 251, 'R')).await.unwrap();
    assert_eq!(paid.balance_after_minor, 1249);
    let frozen_books: Vec<i64> = sqlx::query_scalar(
        "SELECT book_id FROM commerce_layer_term_share_snapshots WHERE term_id=$1",
    )
    .bind(paid.term_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(frozen_books, vec![current_book], "old held book is not in a new paid chain");
    pool.close().await;
}
