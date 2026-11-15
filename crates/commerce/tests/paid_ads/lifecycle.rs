use super::fixture::*;
use tor_contracts::ads::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn accepted_impressions_settle_refund_mature_and_credit_exactly_once() {
    let f = setup(101, 4, 0, 100).await;
    let receipt = f.posting.purchase(&f.command(b'F')).await.unwrap();
    let assignment = f.assign(receipt.campaign_id).await;
    for n in 0..2 {
        let claim = f.opportunity(assignment, n).await;
        let reserved = f.posting.reserve(&claim).await.unwrap();
        assert_eq!(reserved.gross.get(), 25);
        assert!(f.posting.reserve(&claim).await.unwrap().replayed);
    }
    let (a, b) = tokio::join!(
        f.posting.settle(receipt.campaign_id, 100),
        f.posting.settle(receipt.campaign_id, 100)
    );
    assert_eq!(a.unwrap() + b.unwrap(), 2);
    let cancel = AdCancelCommand {
        actor: f.buyer.clone(),
        product: AdProduct::Chapter,
        campaign_id: receipt.campaign_id,
        expected_revision: 1,
        command_key: key(b'G'),
        reason: "Unused impressions".into(),
    };
    assert_eq!(f.posting.cancel(&cancel).await.unwrap().refunded.get(), 51);
    assert_eq!(f.balance(f.buyer.user_id).await, 9950);
    assert_eq!(
        f.posting
            .mature_publisher(&f.publisher, 100)
            .await
            .unwrap()
            .get(),
        35
    );
    let first_key = key(b'I');
    let second_key = key(b'J');
    let (a, b) = tokio::join!(
        f.posting.release_publisher(&f.publisher, &first_key, 100),
        f.posting.release_publisher(&f.publisher, &second_key, 100)
    );
    assert_eq!(a.unwrap().get() + b.unwrap().get(), 35);
    assert_eq!(f.balance(f.publisher.user_id).await, 35);
    assert_eq!(
        f.posting
            .release_publisher(&f.publisher, &key(b'K'), 100)
            .await
            .unwrap()
            .get(),
        0
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT sum(e.delta_minor)::BIGINT FROM finance_ad_entries e
  JOIN finance_ad_transactions t ON t.id=e.transaction_id JOIN finance_ad_funding f ON f.intent_id=t.intent_id
  WHERE f.product='chapter' AND f.campaign_id=$1 AND e.account='platform'")
  .bind(receipt.campaign_id.get()).fetch_one(&f.pool).await.unwrap(),15);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn twenty_parallel_events_at_98_of_100_accept_exactly_two() {
    let f = setup(101, 100, 259200, 1000).await;
    let receipt = f.posting.purchase(&f.command(b'H')).await.unwrap();
    let assignment = f.assign(receipt.campaign_id).await;
    for n in 0..98 {
        f.posting
            .reserve(&f.opportunity(assignment, n).await)
            .await
            .unwrap();
    }
    let mut tasks = Vec::new();
    for n in 98..118 {
        let claim = f.opportunity(assignment, n).await;
        let posting = f.posting.clone();
        tasks.push(tokio::spawn(async move { posting.reserve(&claim).await }));
    }
    let mut accepted = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => accepted += 1,
            Err(AdError::CapReached) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(accepted, 2);
    assert_eq!(
        f.posting.settle(receipt.campaign_id, 100).await.unwrap(),
        100
    );
    assert_eq!(
        f.posting
            .mature_publisher(&f.publisher, 100)
            .await
            .unwrap()
            .get(),
        0
    );
    assert_eq!(
        f.posting
            .release_publisher(&f.publisher, &key(b'K'), 100)
            .await
            .unwrap()
            .get(),
        0
    );
}
