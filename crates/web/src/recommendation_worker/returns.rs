//! Retired collection gates and maintenance over previously saved C cohorts.
use tor_reading::repositories::returns::PgReadingReturns;
use tor_reading::services::returns::ReadingReturnsError;

/// Old environment variables are deliberately inert in the v2 runtime.
pub fn reading_returns_enabled() -> bool {
    false
}

pub fn book_reading_returns_enabled() -> bool {
    false
}

/// Close legacy observation intervals without publishing a new heartbeat.
/// The caller bounds startup errors; optional analytics cannot block HTTP.
pub(crate) async fn initialize_retired_returns(
    store: &PgReadingReturns,
) -> Result<(), ReadingReturnsError> {
    store.start_collection(false).await?;
    store.start_book_collection(false).await
}

/// FALSE only closes coverage, finalizes saved flags and applies existing TTL.
/// These C methods never consume outbox events or collect new reading activity.
pub(crate) async fn maintain_retired_returns(
    store: &PgReadingReturns,
) -> Result<(), ReadingReturnsError> {
    store.tick_collection(false).await?;
    store.tick_book_collection(false).await
}
