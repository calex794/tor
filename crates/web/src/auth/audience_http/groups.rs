//! Fresh administrator authorization for comparison of current public labels.
use super::{administrator, secure, AudienceState};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_recommendations::services::audience::{
    groups::{GroupDiscoveryKind, GroupDiscoveryQuery},
    DiscoveryError,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GroupsQuery {
    days: Option<i32>,
    kind: Option<String>,
    after: Option<String>,
}

impl GroupsQuery {
    fn validated(self) -> Result<GroupDiscoveryQuery, DiscoveryError> {
        let kind = match self.kind.as_deref().unwrap_or("genres") {
            "genres" => GroupDiscoveryKind::Genres,
            "tags" => GroupDiscoveryKind::Tags,
            _ => return Err(DiscoveryError::Invalid),
        };
        GroupDiscoveryQuery::new(self.days.unwrap_or(7), kind, self.after)
    }
}

pub(super) async fn page(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    query: Result<Query<GroupsQuery>, QueryRejection>,
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
    let report = match timeout_at(deadline, state.discovery.groups_report(&query)).await {
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
        super::render::groups::page(&query, report.as_ref()),
    ))
}
