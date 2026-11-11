//! Current public-book aggregates from existing C facts, with fresh A authority.
use super::{administrator, secure, AudienceState};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_reading::services::audience::{
    canonical::{CanonicalDimension, CanonicalQuery},
    AudienceError,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadingQuery {
    days: Option<i32>,
    kind: Option<String>,
    genre: Option<String>,
    tag: Option<String>,
    without_genre: Option<bool>,
    without_tag: Option<bool>,
    after_id: Option<i64>,
    after_label: Option<String>,
}
impl ReadingQuery {
    fn validated(self) -> Result<CanonicalQuery, AudienceError> {
        CanonicalQuery {
            days: self.days.unwrap_or(7),
            dimension: CanonicalDimension::parse(self.kind.as_deref().unwrap_or("books"))?,
            genre: self.genre,
            tag: self.tag,
            without_genre: self.without_genre.unwrap_or(false),
            without_tag: self.without_tag.unwrap_or(false),
            after_id: self.after_id,
            after_label: self.after_label,
        }
        .validated()
    }
}
pub(super) async fn page(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    query: Result<Query<ReadingQuery>, QueryRejection>,
) -> Response {
    let deadline = Instant::now() + Duration::from_millis(950);
    match timeout_at(deadline, administrator(&state, &headers)).await {
        Ok(Ok(())) => {}
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    let query = match query
        .map_err(|_| AudienceError::Invalid)
        .and_then(|Query(q)| q.validated())
    {
        Ok(q) => q,
        Err(_) => return secure(StatusCode::BAD_REQUEST),
    };
    let report = match timeout_at(deadline, state.store.canonical_report(&query)).await {
        Ok(Ok(report)) => Some(report),
        Ok(Err(AudienceError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
        _ => None,
    };
    secure((
        if report.is_some() {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        super::render::canonical::page(&query, report.as_ref()),
    ))
}
