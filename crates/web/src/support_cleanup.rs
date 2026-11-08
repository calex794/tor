use tor_support::repositories::postgres::PgSupport;
pub fn start(repo: PgSupport) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            // Bounded hourly maintenance, no full-table synchronous page scan.
            if repo.cleanup_expired(200).await.is_err() {
                tracing::warn!("support expiry cleanup unavailable");
            }
        }
    })
}
