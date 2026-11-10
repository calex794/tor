//! Separately authorized read-only page over F's current public-book report.
use super::{administrator, secure, AudienceState};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_recommendations::services::audience::{books::BookDiscoveryQuery, DiscoveryError};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BooksQuery {
    days: Option<i32>,
    genre: Option<String>,
    tag: Option<String>,
    after_id: Option<i64>,
}

impl BooksQuery {
    fn validated(self) -> Result<BookDiscoveryQuery, DiscoveryError> {
        BookDiscoveryQuery::new(self.days.unwrap_or(7), self.genre, self.tag, self.after_id)
    }
}

pub(super) async fn page(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    query: Result<Query<BooksQuery>, QueryRejection>,
) -> Response {
    let deadline = Instant::now() + Duration::from_millis(950);
    match timeout_at(deadline, administrator(&state, &headers)).await {
        Ok(Ok(())) => {}
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    let query = match query
        .map_err(|_| DiscoveryError::Invalid)
        .and_then(|Query(query)| query.validated())
    {
        Ok(query) => query,
        Err(_) => return secure(StatusCode::BAD_REQUEST),
    };
    let report = match timeout_at(deadline, state.discovery.books_report(&query)).await {
        Ok(Ok(report)) => Some(report),
        Ok(Err(DiscoveryError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
        _ => None,
    };
    secure((
        if report.is_some() {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        super::render::books::page(&query, report.as_ref()),
    ))
}
