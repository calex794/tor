//! C-owned retained chapter evidence; shares the caller's authorization deadline.
use super::{secure, AudienceState};
use axum::{http::StatusCode, response::Response};
use tokio::time::{timeout_at, Instant};
use tor_reading::services::audience::AudienceError;

pub(super) async fn page(
    state: &AudienceState,
    days: i32,
    zone: &str,
    deadline: Instant,
) -> Response {
    let report = match timeout_at(deadline, state.store.time_report(days, zone)).await {
        Ok(Ok(report)) => Some(report),
        Ok(Err(AudienceError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
        _ => None,
    };
    let status = if report.is_some() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    secure((status, super::render::reading_hours::page(days, zone, report.as_ref())))
}
