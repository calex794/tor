//! Read-only signal journal, independently authorized on every request.
use super::{administrator, secure, AudienceState};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_recommendations::{
    repositories::flow_signals::PgFlowSignals,
    services::flow_signals::SignalError,
};
mod render;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    days: Option<i32>,
    before: Option<i64>,
}
pub(super) async fn page(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    query: Result<Query<Selection>, QueryRejection>,
) -> Response {
    let deadline = Instant::now() + Duration::from_millis(950);
    match timeout_at(deadline, administrator(&state, &headers)).await {
        Ok(Ok(())) => (),
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    let q = match query {
        Ok(Query(q)) => q,
        Err(_) => return secure(StatusCode::BAD_REQUEST),
    };
    let days = q.days.unwrap_or(7);
    let store = PgFlowSignals::new(state.accounts.pool().clone());
    let report = match timeout_at(deadline, store.report(days, q.before)).await {
        Ok(Ok(report)) => Some(report),
        Ok(Err(SignalError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
        _ => None,
    };
    let status = if report.is_some() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    secure((status, render::page(days, report.as_ref())))
}
