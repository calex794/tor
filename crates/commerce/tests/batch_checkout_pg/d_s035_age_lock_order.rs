//! P0 S020/D-06: wallet-row lock order against batch buyer-age authority.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn d_s035_wallet_row_precedes_age_row_for_swap_compatibility() {
    use std::time::Duration;
    let f = fixture("batch-wallet-user-order", 1_000).await;
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(f.buyer)
        .execute(&f.pool)
        .await
        .unwrap();

    let mut wallet_guard = f.pool.begin().await.unwrap();
    let before: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1 FOR UPDATE",
    )
    .bind(f.buyer)
    .fetch_one(&mut *wallet_guard)
    .await
    .unwrap();
    assert_eq!(before, 1_000);

    let checkout = PgCheckout::new(f.pool.clone());
    let quote = command(f.buyer, &f.offers[..2], 'O');
    let task = tokio::spawn(async move { checkout.checkout_batch_once(&quote).await });

    let mut waiting = false;
    for _ in 0..80 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_stat_activity
             WHERE usename=current_user AND wait_event_type='Lock'
               AND query LIKE '%reader_wallet_accounts%'
               AND pid<>pg_backend_pid()",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        if blocked > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(waiting, "batch checkout did not reach its wallet lock");

    let mut age_probe = f.pool.begin().await.unwrap();
    let user: i64 = sqlx::query_scalar(
        "SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE NOWAIT",
    )
    .bind(f.buyer)
    .fetch_one(&mut *age_probe)
    .await
    .expect("batch checkout must not lock age before wallet");
    assert_eq!(user, f.buyer);
    age_probe.rollback().await.unwrap();

    wallet_guard.commit().await.unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("wallet release must unblock batch checkout")
        .expect("batch checkout task must join")
        .expect("eligible batch purchase must complete");
    assert_eq!(receipt.purchases.len(), 2);
    assert_eq!(balance(&f.pool, f.buyer).await, 700);
    assert_eq!(count(&f.pool, f.buyer, "purchase").await, 2);
    assert_eq!(count(&f.pool, f.buyer, "journal").await, 2);
    f.pool.close().await;
}
