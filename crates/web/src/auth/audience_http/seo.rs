//! FLOW-07: read-only provider evidence and a coalesced refresh request, admin only.
use super::{administrator, secure, AudienceState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{
        rejection::{FormRejection, QueryRejection},
        Form, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_search::{
    domain::seo_analytics::{Provider, SeoAnalyticsError},
    repositories::seo_analytics::PgSeoAnalytics,
};
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    days: Option<i32>,
    provider: Option<String>,
    after: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Refresh {
    provider: String,
    csrf_token: String,
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
    let Some(provider) = Provider::parse(q.provider.as_deref().unwrap_or("google")) else {
        return secure(StatusCode::BAD_REQUEST);
    };
    let store = PgSeoAnalytics::new(state.accounts.pool().clone());
    let report = match timeout_at(deadline, store.report(days, provider, q.after.as_deref())).await
    {
        Ok(Ok(r)) => Some(r),
        Ok(Err(SeoAnalyticsError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
        _ => None,
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let status = if report.is_some() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    secure(private_response(
        (
            status,
            super::render::seo::page(days, provider, report.as_ref(), &csrf),
        ),
        &cookies,
    ))
}
pub(super) async fn refresh(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    form: Result<Form<Refresh>, FormRejection>,
) -> Response {
    let deadline = Instant::now() + Duration::from_millis(950);
    match timeout_at(deadline, administrator(&state, &headers)).await {
        Ok(Ok(())) => (),
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    let body = match form {
        Ok(Form(b)) => b,
        Err(_) => return secure(StatusCode::BAD_REQUEST),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &body.csrf_token)
    {
        return secure(StatusCode::FORBIDDEN);
    }
    let Some(provider) = Provider::parse(&body.provider) else {
        return secure(StatusCode::BAD_REQUEST);
    };
    let store = PgSeoAnalytics::new(state.accounts.pool().clone());
    match timeout_at(deadline, store.request_refresh(provider)).await {
        Ok(Ok(_)) => secure(Redirect::to(&format!(
            "/admin/audience/seo?provider={}",
            provider.key()
        ))),
        _ => secure(StatusCode::SERVICE_UNAVAILABLE),
    }
}
