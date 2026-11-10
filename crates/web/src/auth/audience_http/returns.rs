//! A fresh site-administrator check precedes every aggregate read.
use super::{administrator, secure, AudienceState};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_reading::services::returns::ReadingReturnsError;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReturnsQuery {
    days: Option<i32>,
}

pub(super) async fn page(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    query: Result<Query<ReturnsQuery>, QueryRejection>,
) -> Response {
    let deadline = Instant::now() + Duration::from_millis(950);
    match timeout_at(deadline, administrator(&state, &headers)).await {
        Ok(Ok(())) => {}
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    let days = match query {
        Ok(Query(query)) => match query.days.unwrap_or(90) {
            days @ (1 | 7 | 30 | 90) => days,
            _ => return secure(StatusCode::BAD_REQUEST),
        },
        Err(_) => return secure(StatusCode::BAD_REQUEST),
    };
    let report = match timeout_at(deadline, state.returns.returns_report(days)).await {
        Ok(Ok(report)) => Some(report),
        Ok(Err(ReadingReturnsError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
        _ => None,
    };
    secure((
        if report.is_some() {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        super::render::returns::page(days, report.as_ref()),
    ))
}
