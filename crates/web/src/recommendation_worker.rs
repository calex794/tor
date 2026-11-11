//! Only the versioned metadata/manual builder. Retired analytics consumers stay absent.
mod returns;
pub use returns::{book_reading_returns_enabled, reading_returns_enabled};
use std::time::Duration;
use tor_accounts::repositories::PgAccounts;
use tor_recommendations::repositories::PgRecommendations;

/// Called once by the process entry point, never while constructing an HTTP router.
/// An administrator must explicitly create a building generation; idle work is read-only.
pub fn start_contextual(accounts: Option<PgAccounts>) -> Option<tokio::task::JoinHandle<()>> {
    let store = PgRecommendations::new(accounts?.pool().clone());
    Some(tokio::spawn(async move {
        loop {
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
