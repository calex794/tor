//! F-09: recover registrations committed before a process crash.
//! The same restricted PostgreSQL pool is shared with the account runtime.
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};
use tor_referrals::repositories::PgReferrals;

/// Starts only after HTTP bind in authenticated PostgreSQL mode.
/// A missed polling tick never launches an overlapping unbounded task.
pub fn start_referral_consumer(repo: PgReferrals) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(20));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            // In a multi-instance deployment, each row is serialized by
            // finalize_one's row lock; rerunning is safe and cannot pay money.
            if repo.finalize_pending(50).await.is_err() {
                // Do not expose SQL payloads, session hashes or invite codes.
                tracing::warn!("referral signup recovery unavailable");
            }
        }
    })
}
