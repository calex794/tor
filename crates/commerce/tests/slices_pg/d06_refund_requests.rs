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
    // A real D-08 checkout is the only positive source of creator credits.
    // This fixture explicitly seeds *test-only* finance and refund policies.
    let terms_version = format!("ci:terms:{buyer}");
    sqlx::query(
        "INSERT INTO finance_revenue_terms
         (book_id,policy_version,platform_fee_bps,refund_policy_version,refund_window_seconds)
         VALUES($1,$2,2000,$3,86400)",
    )
    .bind(book)
    .bind(&terms_version)
    .bind(&policy)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO finance_revenue_term_states(book_id,policy_version)
         VALUES($1,$2)",
    )
    .bind(book)
    .bind(&terms_version)
    .execute(&pool)
    .await
    .unwrap();
    let purchase = PgCheckout::new(pool.clone())
        .with_creator_posting(true)
        .checkout_once(&command(buyer, chapter, version, 300, 'S'))
        .await
        .unwrap()
        .purchase_id
        .get();
    let recorded_finance: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM finance_chapter_postings WHERE purchase_id=$1")
            .bind(purchase)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(recorded_finance, 1);
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
    // Issuing the first protected key AFTER a pending request must disable
    // a new request; the original idempotent receipt remains unchanged.
    use tor_commerce::repositories::direct_exposure::{DirectGrantKind, PgDirectFirstExposure};
    PgDirectFirstExposure::new(pool.clone())
        .record(
            PositiveId::new(buyer).unwrap(),
            PositiveId::new(chapter).unwrap(),
            PositiveId::new(version).unwrap(),
            Some(PositiveId::new(purchase).unwrap()),
            DirectGrantKind::Chapter,
        )
        .await
        .unwrap();
    let after_grant_key = IdempotencyKey::parse(&format!("{}8", "Y".repeat(42))).unwrap();
    assert_eq!(
        requests
            .request_once(buyer, purchase, &after_grant_key, "новая заявка")
            .await,
        Err(DirectRefundRequestError::AlreadyOpened)
    );
    let replay_after_grant = requests
        .request_once(buyer, purchase, &key, "передумал")
        .await
        .unwrap();
    assert_eq!(replay_after_grant.request_id, first.request_id);
    assert!(!replay_after_grant.created);
    // Reject is an authenticated terminal audit transition, not money.
    use tor_commerce::repositories::refund_decisions::{
        DirectRefundRejectError, PgDirectRefundDecisions,
    };
    let decisions = PgDirectRefundDecisions::new(pool.clone());
    assert_eq!(
        decisions
            .reject_once(buyer, first.request_id, 1, "not eligible")
            .await,
        Err(DirectRefundRejectError::Forbidden)
    );
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',true)",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    let (one, two) = tokio::join!(
        decisions.reject_once(owner, first.request_id, 1, "already opened"),
        decisions.reject_once(owner, first.request_id, 1, "already opened")
    );
    let one = one.unwrap();
    let two = two.unwrap();
    assert_eq!(one.request_id, first.request_id);
    assert_eq!(two.request_id, first.request_id);
    assert_eq!(one.revision, 2);
    assert_eq!(two.revision, 2);
    assert_ne!(one.replayed, two.replayed);
    assert_eq!(
        decisions
            .reject_once(owner, first.request_id, 1, "different reason")
            .await,
        Err(DirectRefundRejectError::Conflict)
    );
    let (state, rev, actor_in_audit): (String, i64, i64) = sqlx::query_as(
        "SELECT state,revision,decision_actor_id
         FROM finance_direct_refund_requests WHERE id=$1",
    )
    .bind(first.request_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (state.as_str(), rev, actor_in_audit),
        ("rejected", 2, owner)
    );
    assert!(sqlx::query(
        "UPDATE finance_direct_refund_requests SET state='pending',
         revision=revision+1 WHERE id=$1"
    )
    .bind(first.request_id)
    .execute(&pool)
    .await
    .is_err());
    // Requests and rejected decisions never settle money or change access.
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
