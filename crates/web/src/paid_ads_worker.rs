//! G orchestration: restart-safe PG fences and exact D operations; no SQL here.
use std::time::Duration;
use tor_accounts::repositories::PgAccounts;
use tor_ads::repositories::paid::PgPaidAds;
use tor_commerce::repositories::ad_posting::PgAdPosting;
use tor_contracts::ads::AdError;
use tor_ingest::repositories::outbox::PgOutbox;

pub fn start(accounts: Option<PgAccounts>) -> Option<tokio::task::JoinHandle<()>> {
    let accounts = accounts?;
    let pool = accounts.pool().clone();
    let store = PgPaidAds::new(pool.clone());
    let posting = PgAdPosting::new(pool.clone());
    let outbox = PgOutbox::new(pool);
    let flags = crate::paid_ads_http::flags();
    Some(tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let result = async {
                // A committed cancellation is a refund obligation even after sales and
                // serving switches are disabled. This can never create a new wallet debit.
                posting.resume_cancellations(8).await?;
                for _ in 0..16 {
                    let Some(event) = outbox
                        .claim_next_for_consumer("AdCampaignPurchased", "ads_aggregate")
                        .await
                        .map_err(|_| AdError::Unavailable)?
                    else {
                        break;
                    };
                    match store
                        .accept_purchase_event(&event.id, event.lease_epoch)
                        .await
                    {
                        Ok(()) => {
                            outbox
                                .acknowledge(&event.id, event.lease_epoch, "ads_aggregate")
                                .await
                                .map_err(|_| AdError::Unavailable)?;
                        }
                        Err(e) => {
                            let _ = outbox
                                .record_failure(
                                    &event.id,
                                    event.lease_epoch,
                                    matches!(e, AdError::Unavailable | AdError::Conflict),
                                )
                                .await;
                            return Err(e);
                        }
                    }
                }
                if flags.planner {
                    match store.plan_batch(50).await {
                        Ok(_) | Err(AdError::Conflict) => {}
                        Err(e) => return Err(e),
                    }
                }
                if flags.settlement {
                    posting.settle_due(100).await?;
                    posting.mature_due(100).await?;
                }
                store.reconcile_outcomes(32).await?;
                store.cleanup().await?;
                Ok::<_, AdError>(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(?error, "paid advertising background batch unavailable");
            }
        }
    }))
}
