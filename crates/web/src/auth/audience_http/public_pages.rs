//! This GET reads saved public-page checks; it never performs a network probe.
use super::{administrator, secure, AudienceState};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_search::{repositories::public_pages::PgPublicPages, services::public_pages::PublicPageProbe};
mod render;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {}
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
    if query.is_err() { return secure(StatusCode::BAD_REQUEST); }
    let report = if let Ok(origin) = PublicPageProbe::new(&state.cookies.origin) {
        let store = PgPublicPages::new(state.accounts.pool().clone());
        timeout_at(deadline, store.report(origin.origin())).await.ok().and_then(Result::ok)
    } else { None };
    let status = if report.is_some() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    secure((status, render::page(report.as_ref())))
}
