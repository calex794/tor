use super::*;

/// The only approved policy below is a test fixture, not a real product
/// policy. It must never be inserted by a production migration.
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_future_policy_refund_request_idempotent_without_money_reversal() {
    use tor_commerce::repositories::refund_requests::{
        DirectRefundRequestError, PgDirectRefundRequests,
    };

    let (pool, buyer, book, chapter, version) =
        fixture("d06_future_policy_request", 1000, 300).await;
    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    let policy = format!("ci:direct-refund:{buyer}");
    sqlx::query(
        "INSERT INTO finance_direct_refund_policy_catalog
         (version,approval_reference,approved_by_user_id,approved_at,effective_at,window_seconds)
         VALUES($1,$2,$3,now()-INTERVAL '2 days',now()-INTERVAL '1 day',86400)",
    )
    .bind(&policy)
    .bind(format!("ci:d06:approval:{buyer}"))
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    // New policy-bound purchase, with exactly one buyer debit and a frozen
    // B-10 split. This fixture uses SQL only to exercise an *unreleased*
    // purchase-time policy; the regular checkout never binds a refund policy.
    let reference = format!("ci:d06:wallet:{buyer}:{chapter}");
    let purchase: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
         (user_id,chapter_id,version_id,settlement_reference,
          amount_minor,currency,refund_policy_version,refund_deadline_at)
         VALUES($1,$2,$3,$4,300,'RUB',$5,now()+INTERVAL '1 day')
         RETURNING id",
    )
    .bind(buyer)
    .bind(chapter)
    .bind(version)
    .bind(&reference)
    .bind(&policy)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_share_snapshots
         (purchase_id,book_id,share_revision,gross_amount_minor)
         SELECT $1,$2,current_revision,300
         FROM revenue_share_states WHERE book_id=$2",
    )
    .bind(purchase)
    .bind(book)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_share_lines(purchase_id,user_id,share_percent)
         SELECT $1,a.user_id,a.share_percent
         FROM revenue_share_assignments a
         JOIN revenue_share_states s
           ON s.book_id=a.book_id AND s.current_revision=a.revision
         WHERE a.book_id=$2",
    )
    .bind(purchase)
    .bind(book)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=700 WHERE user_id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_entries
         (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
         VALUES($1,'chapter_purchase',-300,700,$2,$3)",
    )
    .bind(buyer)
    .bind(&reference)
    .bind(purchase)
    .execute(&pool)
    .await
    .unwrap();
    let requests = PgDirectRefundRequests::new(pool.clone());
    let key = IdempotencyKey::parse(&format!("{}8", "S".repeat(42))).unwrap();
    let other = IdempotencyKey::parse(&format!("{}8", "T".repeat(42))).unwrap();
    assert_eq!(
        requests
            .request_once(buyer + 1, purchase, &key, "передумал")
            .await,
        Err(DirectRefundRequestError::NotFound)
    );
    let first = requests
        .request_once(buyer, purchase, &key, "передумал")
        .await
        .unwrap();
    assert!(first.created);
    let repeat = requests
        .request_once(buyer, purchase, &key, "передумал")
        .await
        .unwrap();
    assert_eq!(repeat.request_id, first.request_id);
    assert!(!repeat.created);
    assert_eq!(
        requests
            .request_once(buyer, purchase, &key, "другая причина")
            .await,
        Err(DirectRefundRequestError::DuplicateKeyDifferentRequest)
    );
    assert_eq!(
        requests
            .request_once(buyer, purchase, &other, "передумал")
            .await,
        Err(DirectRefundRequestError::AlreadyOpen)
    );
    let stored: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM finance_direct_refund_requests WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, 1);
    // Requests never settle money or revoke paid access.
    assert_eq!(count(&pool, "ledger", buyer).await, 1);
    assert_eq!(count(&pool, "rights", buyer).await, 1);
    assert_eq!(count(&pool, "facts", buyer).await, 1);
    let wallet: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(wallet, 700);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_real_checkout_never_automatically_activates_refunds() {
    use tor_commerce::repositories::refund_requests::{
        DirectRefundRequestError, PgDirectRefundRequests,
    };

    let (pool, actor, _, chapter, version) =
        fixture("d06_legacy_request_disabled", 1000, 100).await;
    let purchase = PgCheckout::new(pool.clone())
        .checkout_once(&command(actor, chapter, version, 100, 'V'))
        .await
        .unwrap()
        .purchase_id
        .get();
    let key = IdempotencyKey::parse(&format!("{}8", "W".repeat(42))).unwrap();
    let result = PgDirectRefundRequests::new(pool.clone())
        .request_once(actor, purchase, &key, "не открывал")
        .await;
    assert_eq!(result, Err(DirectRefundRequestError::PolicyUnavailable));
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM finance_direct_refund_requests WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}
