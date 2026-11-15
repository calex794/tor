use tor_accounts::repositories::PgAccounts;

/// Spawned explicitly by the server entry point, never by Router construction.
pub async fn start(accounts: Option<PgAccounts>) -> Option<tokio::task::JoinHandle<()>> {
    let accounts = accounts?;
    let preferences =
        tor_recommendations::repositories::PgRecommendations::new(accounts.pool().clone());
    // The SQL migration closes old coverage. HTTP never waits for a global
    // drain, and no old generation/consumer/finalizer is initialized here.
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
                let mut timer = tokio::time::interval(std::time::Duration::from_secs(5));
                timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last_pending = None;
                loop {
                    timer.tick().await;
                    // Explicit product choices have their own TTL and CAS.
                    // A blocked/failed legacy erasure cannot stop this cleanup.
                    if !matches!(
                        tokio::time::timeout(
                            std::time::Duration::from_millis(1500),
                            preferences.prune_preferences()
                        )
                        .await,
                        Ok(Ok(_))
                    ) {
                        eprintln!("TOR_RECOMMENDATION_PREFERENCES_RETENTION_UNAVAILABLE");
                    }
                    // F bounds the server statement before entering the SQL
                    // port; the process timeout also bounds optional waiting.
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(1000),
                        preferences.retire_analytics_batch(256),
                    )
                    .await
                    {
                        Ok(Ok((_, pending))) => {
                            if last_pending != Some(pending) {
                                if pending {
                                    eprintln!("TOR_ANALYTICS_RETIREMENT_PENDING");
                                } else {
                                    eprintln!("TOR_ANALYTICS_DATABASE_RETIREMENT_IDLE");
                                }
                            }
                            last_pending = Some(pending);
                        }
                        _ => {
                            last_pending = None;
                            eprintln!("TOR_ANALYTICS_RETIREMENT_UNAVAILABLE");
                        }
                    }
                }
            }
        );
    }))
}
