//! S031: public Layer price and composition are a read-only, versioned quote.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_layer_offer_quote_is_complete_and_never_authoritative_for_checkout() {
    let (pool, repo, owner, _buyer, book) = setup("quote-complete").await;
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
