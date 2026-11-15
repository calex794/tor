use super::fixture::*;
use tor_contracts::ads::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn paid_purchase_replay_conflict_failed_intent_and_wallet_race() {
    let f = setup(101, 4, 0, 100).await;
    let command = f.command(b'A');
    let (a, b) = tokio::join!(f.posting.purchase(&command), f.posting.purchase(&command));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.campaign_id, b.campaign_id);
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(f.balance(f.buyer.user_id).await, 9899);
    let mut changed = command.clone();
    if let AdPurchase::Chapter(ref mut v) = changed.purchase {
        v.expected_package_revision = 2;
    }
    assert_eq!(
        f.posting.purchase(&changed).await,
        Err(AdError::IdempotencyConflict)
    );
    let replay = f.posting.purchase(&command).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.balance_after.get(), 9899);
    // Restarted repository has no process-local idempotency authority.
    let restarted = tor_commerce::repositories::ad_posting::PgAdPosting::new(f.pool.clone());
    assert_eq!(
        restarted.purchase(&command).await.unwrap().campaign_id,
        a.campaign_id
    );
    sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=0 WHERE user_id=$1")
        .bind(f.buyer.user_id)
        .execute(&f.pool)
        .await
        .unwrap();
    let failed = f.command(b'B');
    assert_eq!(
        f.posting.purchase(&failed).await,
        Err(AdError::Insufficient)
    );
    let mut changed = failed.clone();
    if let AdPurchase::Chapter(ref mut v) = changed.purchase {
        v.book_id = f.host;
    }
    assert_eq!(
        f.posting.purchase(&changed).await,
        Err(AdError::IdempotencyConflict)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM finance_ad_funding WHERE advertiser_id=$1"
        )
        .bind(f.buyer.user_id)
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn daily_full_prepayment_overlap_and_exact_unused_days_refund() {
    let f = setup(101, 4, 0, 100).await;
    let command = f.daily(b'C').await;
    let mut credit = command.clone();
    if let AdPurchase::Daily(ref mut v) = credit.purchase {
        v.prepaid_days = 1;
    }
    assert_eq!(f.posting.purchase(&credit).await, Err(AdError::Invalid));
    let receipt = f.posting.purchase(&command).await.unwrap();
    assert_eq!(receipt.charged.get(), 300);
    let mut overlap = command.clone();
    overlap.command_key = key(b'D');
    assert_eq!(f.posting.purchase(&overlap).await, Err(AdError::Conflict));
    let cancel = AdCancelCommand {
        actor: f.buyer.clone(),
        product: AdProduct::Daily,
        campaign_id: receipt.campaign_id,
        expected_revision: 1,
        command_key: key(b'E'),
        reason: "Cancel unused complete UTC days".into(),
    };
    let refunded = f.posting.cancel(&cancel).await.unwrap();
    assert_eq!(refunded.refunded.get(), 200);
    assert!(!refunded.pending);
    assert_eq!(f.balance(f.buyer.user_id).await, 9900);
    assert!(f.posting.cancel(&cancel).await.unwrap().replayed);
    let mut conflict = cancel;
    conflict.reason = "changed".into();
    assert_eq!(
        f.posting.cancel(&conflict).await,
        Err(AdError::IdempotencyConflict)
    );
}
