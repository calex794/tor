//! F-12: existing PostgreSQL outbox, private badge delivery.
//! Failure never invalidates the already committed chapter completion.
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};
use tor_recommendations::repositories::PgAchievements;

pub fn start_achievements_consumer(repo: PgAchievements) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let ids = match repo.pending_completion_event_ids().await {
                Ok(ids) => ids,
                Err(_) => {
                    tracing::warn!("achievement outbox polling unavailable");
                    continue;
                }
            };
            for id in ids {
                if repo.accept_completed_chapter(&id).await.is_err() {
                    tracing::warn!("trusted reader achievement delivery remains queued");
                }
            }
        }
    })
}
