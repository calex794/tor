use tor_accounts::repositories::PgAccounts;

/// Spawned explicitly by the server entry point, never by Router construction.
pub async fn start(accounts: Option<PgAccounts>) -> Option<tokio::task::JoinHandle<()>> {
    let accounts = accounts?;
    let visits =
        tor_recommendations::repositories::visits::PgFlowVisits::new(accounts.pool().clone());
    let funnel =
        tor_recommendations::repositories::flow_funnel::PgFlowFunnel::new(accounts.pool().clone());
    let reading =
        tor_reading::repositories::flow_capture::PgFlowCapture::new(accounts.pool().clone());
    let collect = crate::recommendation_http::visits::collection_enabled();
    // Record this worker's initial mode before serving requests. C's atomic
    // scope receipts authorize attribution, never this diagnostic singleton.
    if !matches!(
        tokio::time::timeout(
            std::time::Duration::from_millis(1000),
            funnel.initialize(collect)
        )
        .await,
        Ok(Ok(()))
    ) {
        eprintln!("TOR_FLOW_FUNNEL_INITIALIZE_UNAVAILABLE");
    }
    Some(tokio::spawn(async move {
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
                    if !matches!(
                        tokio::time::timeout(
                            std::time::Duration::from_millis(2500),
                            funnel.cycle(collect)
                        )
                        .await,
                        Ok(Ok(_))
                    ) {
                        eprintln!("TOR_FLOW_FUNNEL_CYCLE_UNAVAILABLE");
                    }
                    if !matches!(
                        tokio::time::timeout(
                            std::time::Duration::from_millis(1500),
                            reading.maintain()
                        )
                        .await,
                        Ok(Ok(()))
                    ) {
                        eprintln!("TOR_FLOW_READING_RETENTION_UNAVAILABLE");
                    }
                }
            }
        );
    }))
}
