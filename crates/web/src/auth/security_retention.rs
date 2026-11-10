use tor_accounts::repositories::PgAccounts;

/// Spawned explicitly by the server entry point, never by Router construction.
pub fn start(accounts: PgAccounts) -> tokio::task::JoinHandle<()> {
    let visits =
        tor_recommendations::repositories::visits::PgFlowVisits::new(accounts.pool().clone());
    tokio::spawn(async move {
        tokio::join!(
            async move {
                let mut timer = tokio::time::interval(std::time::Duration::from_secs(300));
                timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    timer.tick().await;
                    if accounts.prune_security_data().await.is_err() {
                        eprintln!("TOR_ACCOUNT_RETENTION_UNAVAILABLE");
                    }
                }
            },
            async move {
                // Independent from all collection flags and the slower A cleanup.
                let mut timer = tokio::time::interval(std::time::Duration::from_secs(5));
                timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    timer.tick().await;
                    if !matches!(
                        tokio::time::timeout(
                            std::time::Duration::from_millis(2500),
                            visits.maintain()
                        )
                        .await,
                        Ok(Ok(_))
                    ) {
                        eprintln!("TOR_FLOW_VISIT_RETENTION_UNAVAILABLE");
                    }
                }
            }
        );
    })
}
