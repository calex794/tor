//! Versioned metadata builder and bounded F-SURPRISE safety/retention tick.
//! Retired individual analytics consumers stay absent.
mod returns;
pub use returns::{book_reading_returns_enabled, reading_returns_enabled};
use std::time::{Duration, Instant};
use tor_accounts::repositories::PgAccounts;
use tor_recommendations::repositories::{surprises::PgSurprises, PgRecommendations};

/// Called once by the process entry point, never while constructing an HTTP router.
/// Retention runs even when gifts and experiments are disabled.
pub fn start_contextual(accounts: Option<PgAccounts>) -> Option<tokio::task::JoinHandle<()>> {
    let pool = accounts?.pool().clone();
    let store = PgRecommendations::new(pool.clone());
    let surprises = PgSurprises::new(pool);
    Some(tokio::spawn(async move {
        let mut checked = None::<Instant>;
        loop {
            if checked.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) {
                let enabled = crate::auth::surprise_http::enabled();
                match tokio::time::timeout(
                    Duration::from_millis(2000),
                    surprises.maintain_experiment_with_delivery(enabled),
                )
                .await
                {
                    Ok(Ok(true)) => {}
                    Ok(Ok(false)) => eprintln!("TOR_SURPRISE_RETENTION_BACKLOG"),
                    _ => {
                        eprintln!("TOR_SURPRISE_MAINTENANCE_UNAVAILABLE");
                        if !matches!(
                            tokio::time::timeout(
                                Duration::from_millis(2000),
                                surprises.recover_expired_experiment(),
                            )
                            .await,
                            Ok(Ok(()))
                        ) {
                            eprintln!("TOR_SURPRISE_RETENTION_UNAVAILABLE");
                        }
                    }
                }
                checked = Some(Instant::now());
            }
            let delay = match tokio::time::timeout(
                Duration::from_millis(2000),
                store.advance_contextual_generation(),
            )
            .await
            {
                Ok(Ok(true)) => Duration::from_millis(250),
                Ok(Ok(false)) => Duration::from_secs(5),
                _ => {
                    eprintln!("TOR_CONTEXTUAL_BUILD_UNAVAILABLE");
                    Duration::from_secs(5)
                }
            };
            tokio::time::sleep(delay).await;
        }
    }))
}
