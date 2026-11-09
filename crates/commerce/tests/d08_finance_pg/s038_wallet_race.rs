//! Concurrent S038 payout and D-02 checkout regression on disposable PostgreSQL.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s038_concurrent_checkout_and_payout_reservation_have_one_winner() {
    let f = fixture("s038_checkout_reservation_race").await;
    publish_terms(&f, "ci-s038-payout-race-v1", 2000).await;
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',true)",
    )
    .bind(f.book)
    .bind(f.buyer)
    .execute(&f.pool)
    .await
    .unwrap();
    let finance = PgCreatorFinance::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    let key = IdempotencyKey::parse(&format!("{}8", "Q".repeat(42))).unwrap();
    let command = buy_one(&f, f.offers[0], 'R');

    // The buyer's 1000 cannot satisfy BOTH a 900 pending reservation
    // and a 250 purchase. In particular, FK KEY SHARE on users must not
    // deadlock against the wallet trigger's non-key users row lock.
    let (reservation, purchase) = tokio::time::timeout(
        std::time::Duration::from_secs(25),
        async {
            tokio::join!(
                finance.reserve_once(f.buyer, 900, &key),
                checkout.checkout_once(&command)
            )
        },
    )
    .await
    .expect("payout-vs-checkout deadlock");
    assert_eq!(
        reservation.is_ok() as u8 + purchase.is_ok() as u8,
        1,
        "exactly one use of shared funds can succeed: reservation={reservation:?}, purchase={purchase:?}"
    );
    if reservation.is_ok() {
        assert_eq!(purchase, Err(CheckoutError::InsufficientBalance));
        assert_eq!(balance(&f).await, 1000);
        assert_eq!(finance.totals(f.buyer).await.unwrap().reserved_minor, 900);
    } else {
        assert_eq!(reservation, Err(CreatorFinanceError::Insufficient));
        assert!(purchase.is_ok());
        assert_eq!(balance(&f).await, 750);
        assert_eq!(finance.totals(f.buyer).await.unwrap().reserved_minor, 0);
    }
    assert_eq!(
        finance.totals(f.buyer).await.unwrap().available_minor,
        if reservation.is_ok() { 100 } else { 750 }
    );
    f.pool.close().await;
}
