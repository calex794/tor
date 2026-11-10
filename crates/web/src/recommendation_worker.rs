//! I-owned maintenance for retired optional REC/FLOW collection.
//! No outbox consumer or generation builder runs from the production server.
use tor_recommendations::{domain::feedback::FeedbackError, repositories::PgRecommendations};
mod returns;
pub use returns::{book_reading_returns_enabled, reading_returns_enabled};
pub(crate) use returns::{initialize_retired_returns, maintain_retired_returns};

/// Reconcile already queued erasures before ordinary TTL pruning. These
/// operations read retained REC rows only; they never acknowledge C events.
/// P1 preference cleanup also runs independently of this legacy maintenance.
pub(crate) async fn maintain_legacy_recommendations(
    store: &PgRecommendations,
) -> Result<(), FeedbackError> {
    store.refresh_quality().await?;
    store.refresh_organic_quality().await?;
    store.prune().await?;
    Ok(())
}
