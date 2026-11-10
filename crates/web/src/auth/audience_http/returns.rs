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
    book_id: Option<i64>,
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
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => return secure(StatusCode::BAD_REQUEST),
    };
    let days = match query.days.unwrap_or(90) {
        days @ (1 | 7 | 30 | 90) => days,
        _ => return secure(StatusCode::BAD_REQUEST),
    };
    if let Some(book) = query.book_id {
        if book <= 0 {
            return secure(StatusCode::BAD_REQUEST);
        }
        let report = match timeout_at(deadline, state.returns.book_returns_report(book, days)).await
        {
            Ok(Ok(Some(report))) => Some(report),
            Ok(Ok(None)) => return secure(StatusCode::NOT_FOUND),
            Ok(Err(ReadingReturnsError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
            _ => None,
        };
        return secure((
            if report.is_some() {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            super::render::book_returns::page(book, days, report.as_ref()),
        ));
    }
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
