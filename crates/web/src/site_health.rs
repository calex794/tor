//! Liveness and readiness HTTP handlers.
use super::AppState;
use axum::{extract::State, http::StatusCode};

pub(super) async fn health() -> &'static str {
    "ok"
}
pub(super) async fn ready(State(state): State<AppState>) -> (StatusCode, &'static str) {
    if let Some(catalog) = state.postgres {
        if catalog.probe().await.is_err() {
            return (StatusCode::SERVICE_UNAVAILABLE, "unavailable");
        }
    }
    if let Some(reader) = state.reader {
        if reader.probe().await.is_err() {
            return (StatusCode::SERVICE_UNAVAILABLE, "unavailable");
        }
    }
    (StatusCode::OK, "ok") // demo mode has neither PG adapter
}
