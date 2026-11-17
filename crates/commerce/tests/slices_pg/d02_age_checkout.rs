//! P0 S020/D: buyer-age entitlement at debit, distinct from paid possession.
//! Uses only disposable /tor_ci PostgreSQL, never real accounts or wallets.
use super::*;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_adult_and_suspended_buyer_never_charge_without_live_eligibility() {
    let (pool, buyer, book, chapter, version) = fixture("adult_purchase_age", 900, 100).await;
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let store = PgCheckout::new(pool.clone());
    let buy = command(buyer, chapter, version, 100, 'V');

    assert_eq!(
        store.checkout_once(&buy).await,
        Err(CheckoutError::Inaccessible)
    );
    money_unchanged(&pool, buyer, 900).await;

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    let paid = store.checkout_once(&buy).await.unwrap();
    assert!(!paid.replayed);
    assert_eq!(paid.balance_after.get(), 800);
    let can_read: bool = sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
        .bind(buyer)
        .bind(paid.purchase_id.get())
        .bind(chapter)
        .bind(version)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(can_read);

    // Losing age clearance later does not delete immutable paid possession.
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    let now_readable: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(buyer)
            .bind(paid.purchase_id.get())
            .bind(chapter)
            .bind(version)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!now_readable);
    assert!(store.checkout_once(&buy).await.unwrap().replayed);
    assert_eq!(count(&pool, "facts", buyer).await, 1);
    assert_eq!(count(&pool, "rights", buyer).await, 1);
    assert_eq!(count(&pool, "ledger", buyer).await, 1);

    let (closed_pool, closed, _, closed_ch, closed_v) =
        fixture("suspended_purchase_age", 900, 100).await;
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(closed)
        .execute(&closed_pool)
        .await
        .unwrap();
    assert_eq!(
        PgCheckout::new(closed_pool.clone())
            .checkout_once(&command(closed, closed_ch, closed_v, 100, 'W'))
            .await,
        Err(CheckoutError::Inaccessible)
    );
    money_unchanged(&closed_pool, closed, 900).await;
    pool.close().await;
    closed_pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_pro_adult_needs_own_active_grant_and_preserves_paid_history() {
    let (pool, buyer, book, chapter, version) = fixture("pro_purchase_age", 900, 100).await;
    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let store = PgCheckout::new(pool.clone());
    let buy = command(buyer, chapter, version, 100, 'X');
    assert_eq!(
        store.checkout_once(&buy).await,
        Err(CheckoutError::Inaccessible)
    );
    money_unchanged(&pool, buyer, 900).await;

    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',FALSE)",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store.checkout_once(&buy).await,
        Err(CheckoutError::Inaccessible)
    );
    money_unchanged(&pool, buyer, 900).await;

    sqlx::query(
        "UPDATE user_level_grants SET is_active=TRUE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let paid = store.checkout_once(&buy).await.unwrap();
    assert!(!paid.replayed);
    let entitled: bool = sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
        .bind(buyer)
        .bind(paid.purchase_id.get())
        .bind(chapter)
        .bind(version)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(entitled);

    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let after_revocation: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(buyer)
            .bind(paid.purchase_id.get())
            .bind(chapter)
            .bind(version)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!after_revocation);
    assert!(store.checkout_once(&buy).await.unwrap().replayed);
    assert_eq!(count(&pool, "facts", buyer).await, 1);
    assert_eq!(count(&pool, "rights", buyer).await, 1);
    assert_eq!(count(&pool, "ledger", buyer).await, 1);
    pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_adult_revocation_committed_before_buyer_lock_prevents_charge() {
    let (pool, buyer, book, chapter, version) = fixture("adult_revoke_race", 900, 100).await;
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();

    // NO KEY UPDATE does not block purchase-command user-id FK KEY SHARE.
    // It *does* block the age reader's FOR SHARE until revocation commits.
    let mut revoking = pool.begin().await.unwrap();
    let locked: i64 = sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(buyer)
        .fetch_one(&mut *revoking)
        .await
        .unwrap();
    assert_eq!(locked, buyer);
    let buy = command(buyer, chapter, version, 100, 'Y');
    let checkout = PgCheckout::new(pool.clone());
    let mut task = tokio::spawn(async move { checkout.checkout_once(&buy).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut task)
            .await
            .is_err(),
        "purchase must not pass a locked age authority"
    );
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(buyer)
        .execute(&mut *revoking)
        .await
        .unwrap();
    revoking.commit().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .expect("age lock must release")
            .expect("checkout task must join"),
        Err(CheckoutError::Inaccessible)
    );
    money_unchanged(&pool, buyer, 900).await;
    pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_pro_grant_revocation_committed_before_grant_lock_prevents_charge() {
    let (pool, buyer, book, chapter, version) = fixture("pro_revoke_race", 900, 100).await;
    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();

    let mut revoking = pool.begin().await.unwrap();
    let locked: i64 = sqlx::query_scalar(
        "SELECT user_id FROM user_level_grants
         WHERE user_id=$1 AND level='pro_adult' AND is_active FOR UPDATE",
    )
    .bind(buyer)
    .fetch_one(&mut *revoking)
    .await
    .unwrap();
    assert_eq!(locked, buyer);
    let buy = command(buyer, chapter, version, 100, 'Z');
    let checkout = PgCheckout::new(pool.clone());
    let mut task = tokio::spawn(async move { checkout.checkout_once(&buy).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut task)
            .await
            .is_err(),
        "purchase must wait for live pro-adult grant"
    );
    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(buyer)
    .execute(&mut *revoking)
    .await
    .unwrap();
    revoking.commit().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .expect("age grant lock must release")
            .expect("checkout task must join"),
        Err(CheckoutError::Inaccessible)
    );
    money_unchanged(&pool, buyer, 900).await;
    pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_02_wallet_row_must_precede_age_row_for_swap_lock_order() {
    let (pool, buyer, book, chapter, version) =
        fixture("adult_wallet_user_order", 900, 100).await;
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();

    // Hold the wallet, as an in-flight D-06 swap does. The new checkout
    // must reach this lock BEFORE taking users FOR SHARE, otherwise S038's
    // wallet -> users trigger can deadlock with the direct checkout path.
    let mut wallet_guard = pool.begin().await.unwrap();
    let existing: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts
         WHERE user_id=$1 FOR UPDATE",
    )
    .bind(buyer)
    .fetch_one(&mut *wallet_guard)
    .await
    .unwrap();
    assert_eq!(existing, 900);

    let checkout = PgCheckout::new(pool.clone());
    let buy = command(buyer, chapter, version, 100, 'L');
    let task = tokio::spawn(async move { checkout.checkout_once(&buy).await });

    // Establish that the checkout has actually arrived at the blocked
    // wallet acquisition. Without this barrier a fast lock-order probe
    // could pass before the competing transaction reached its age read.
    let mut waiting = false;
    for _ in 0..80 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_stat_activity
             WHERE usename=current_user AND wait_event_type='Lock'
               AND query LIKE '%reader_wallet_accounts%'
               AND pid<>pg_backend_pid()",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if blocked > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(waiting, "checkout did not reach the wallet lock");

    // An update-compatible user lock must remain immediately available
    // while the checkout is waiting for the wallet. If the checkout had
    // locked users SHARE first, NOWAIT would fail deterministically.
    let mut age_probe = pool.begin().await.unwrap();
    let user: i64 = sqlx::query_scalar(
        "SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE NOWAIT",
    )
    .bind(buyer)
    .fetch_one(&mut *age_probe)
    .await
    .expect("buyer age lock must not precede the wallet");
    assert_eq!(user, buyer);
    age_probe.rollback().await.unwrap();

    wallet_guard.commit().await.unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("wallet release must unblock checkout")
        .expect("checkout task must join")
        .expect("eligible buyer must be able to purchase");
    assert!(!receipt.replayed);
    assert_eq!(count(&pool, "facts", buyer).await, 1);
    assert_eq!(count(&pool, "ledger", buyer).await, 1);
    pool.close().await;
}
