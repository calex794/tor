//! Live administrator guard and one deadline for the consented book journey.
use super::{administrator, secure, AudienceQuery, AudienceState};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_recommendations::services::flow_funnel::FlowFunnelError;

pub(super) async fn page(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    query: Result<Query<AudienceQuery>, QueryRejection>,
) -> Response {
    let deadline = Instant::now() + Duration::from_millis(950);
    match timeout_at(deadline, administrator(&state, &headers)).await {
        Ok(Ok(())) => {}
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    let days = match query
        .map_err(|_| StatusCode::BAD_REQUEST)
        .and_then(|Query(query)| query.days())
    {
        Ok(days) => days,
        Err(status) => return secure(status),
    };
    let report = match timeout_at(deadline, state.funnel.report(days)).await {
        Ok(Ok(report)) => Some(report),
        Ok(Err(FlowFunnelError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
        _ => None,
    };
    secure((
        if report.is_some() {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        super::render::funnel::page(days, report.as_ref(), state.capture.health()),
    ))
}
