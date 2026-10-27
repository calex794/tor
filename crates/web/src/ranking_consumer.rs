//! I/F handoff: bounded delivery of C's already committed completion facts.
//! The general G dispatcher may replace the poller; receipt keys are stable.
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};
use tor_recommendations::repositories::{PgRanking, RankingStoreError};

/// Starts after the HTTP listener has been bound and only in authenticated
/// PostgreSQL mode. One shared database pool, no separate migration access.
pub fn start_rankings_consumer(repo: PgRanking) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let ids = match repo.pending_completion_event_ids().await {
                Ok(ids) => ids,
                Err(error) => {
                    tracing::warn!(?error, "ranking completion polling unavailable");
                    continue;
                }
            };
            for id in ids {
                match repo.accept_completed_chapter(&id).await {
                    Ok(_) | Err(RankingStoreError::Conflict) => {}
                    Err(error) => {
                        // Never log actor IDs, chapter bodies, or raw event payloads.
                        tracing::warn!(?error, "verified ranking completion delivery failed");
                    }
                }
            }
        }
    })
}
