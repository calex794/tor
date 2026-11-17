//! P0 D-02/S020: a legacy Book with no attested age tier cannot be sold.
//! Never infer 'basic' from a NULL Book access_level.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_unknown_book_age_tier_refuses_debit_until_attested() {
    let (pool, buyer, book, chapter, version) = fixture("unknown_age_tier", 900, 100).await;
    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let checkout = PgCheckout::new(pool.clone());
    let buy = command(buyer, chapter, version, 100, 'U');

    assert_eq!(
        checkout.checkout_once(&buy).await,
        Err(CheckoutError::Inaccessible)
    );
    money_unchanged(&pool, buyer, 900).await;

    // Once the Book tier is explicitly attested, the same attempted
    // request can complete without a stale command key or duplicate debit.
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let paid = checkout.checkout_once(&buy).await.unwrap();
    assert!(!paid.replayed);
    assert_eq!(paid.balance_after.get(), 800);
    assert_eq!(count(&pool, "facts", buyer).await, 1);
    assert_eq!(count(&pool, "rights", buyer).await, 1);
    assert_eq!(count(&pool, "ledger", buyer).await, 1);
    pool.close().await;
}
