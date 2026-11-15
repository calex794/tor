use super::fixture::*;
use tor_contracts::ads::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn zero_cost_impressions_are_counted_without_fake_minor_unit_debt() {
    let f = setup(1, 4, 0, 100).await;
    let receipt = f.posting.purchase(&f.command(b'L')).await.unwrap();
    let assignment = f.assign(receipt.campaign_id).await;
    for n in 0..3 {
        assert_eq!(
            f.posting
                .reserve(&f.opportunity(assignment, n).await)
                .await
                .unwrap()
                .gross
                .get(),
            0
        );
    }
    assert_eq!(f.posting.settle(receipt.campaign_id, 100).await.unwrap(), 3);
    let cancel = AdCancelCommand {
        actor: f.buyer.clone(),
        product: AdProduct::Chapter,
        campaign_id: receipt.campaign_id,
        expected_revision: 1,
        command_key: key(b'M'),
        reason: "Zero-cost remainder".into(),
    };
    assert_eq!(f.posting.cancel(&cancel).await.unwrap().refunded.get(), 1);
    assert_eq!(f.balance(f.buyer.user_id).await, 10000);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM finance_ad_entries e JOIN finance_ad_transactions t
  ON t.id=e.transaction_id JOIN finance_ad_funding f ON f.intent_id=t.intent_id
  WHERE f.product='chapter' AND f.campaign_id=$1 AND t.operation IN('reserve','settle')"
        )
        .bind(receipt.campaign_id.get())
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        0
    );
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn durable_caps_viewability_and_current_book_acl_fail_closed() {
    let f = setup(101, 4, 0, 1).await;
    let receipt = f.posting.purchase(&f.command(b'N')).await.unwrap();
    let assignment = f.assign(receipt.campaign_id).await;
    let mut claim = f.opportunity(assignment, 0).await;
    claim.visible_percent = 59;
    assert_eq!(f.posting.reserve(&claim).await, Err(AdError::Invalid));
    claim.visible_percent = 60;
    claim.duration_ms = 900;
    assert_eq!(f.posting.reserve(&claim).await, Err(AdError::Invalid));
    claim.duration_ms = 1000;
    let mut wrong_context = claim.clone();
    wrong_context.context.generation_id =
        AdGenerationId::new(claim.context.generation_id.get() + 1000).unwrap();
    assert_eq!(
        f.posting.reserve(&wrong_context).await,
        Err(AdError::Denied)
    );
    let mut forged = claim.clone();
    forged.actor = f.publisher.clone();
    assert_eq!(f.posting.reserve(&forged).await, Err(AdError::Denied));
    f.posting.reserve(&claim).await.unwrap();
    let next = f.opportunity(assignment, 1).await;
    let restarted = tor_commerce::repositories::ad_posting::PgAdPosting::new(f.pool.clone());
    assert_eq!(restarted.reserve(&next).await, Err(AdError::CapReached));
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(f.book)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(f.posting.reserve(&next).await, Err(AdError::Denied));
    assert!(f.posting.reserve(&claim).await.unwrap().replayed);
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn concurrent_distinct_orders_cannot_overdraw_and_raw_orphan_campaign_is_rejected() {
    let f = setup(101, 4, 0, 100).await;
    sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=150 WHERE user_id=$1")
        .bind(f.buyer.user_id)
        .execute(&f.pool)
        .await
        .unwrap();
    let c1 = f.command(b'O');
    let c2 = f.command(b'P');
    let (a, b) = tokio::join!(f.posting.purchase(&c1), f.posting.purchase(&c2));
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(AdError::Insufficient)) | (Err(AdError::Insufficient), Ok(_))
    ));
    assert_eq!(f.balance(f.buyer.user_id).await, 49);
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO ad_chapter_campaigns(advertiser_id,promoted_book_id,package_id,package_revision,funding_intent_id,status)
   SELECT advertiser_id,book_id,offer_id,offer_revision,id,'funded' FROM finance_ad_intents
   WHERE advertiser_id=$1 AND status='failed'").bind(f.buyer.user_id).execute(&mut *tx).await.unwrap();
    assert_eq!(
        tx.commit()
            .await
            .unwrap_err()
            .as_database_error()
            .unwrap()
            .code()
            .as_deref(),
        Some("23514")
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ad_chapter_campaigns WHERE advertiser_id=$1")
            .bind(f.buyer.user_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}
