//! After-listener polling of the shared outbox; never starts from test router construction.
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};
use tor_social::repositories::notifications::PgNotifications;

pub fn start(repo: PgNotifications) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(error) = repo.deliver_pending(50).await {
                // No user ID, message body, profile name or event payload in logs.
                tracing::warn!(?error, "on-site social notification delivery unavailable");
            }
        }
    })
}
