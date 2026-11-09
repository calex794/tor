//! FLOW: site-administrator access to canonical C reading aggregates.
//! No tracking identifier, collection switch, SQL or operator-password fallback.
mod render;

use crate::auth_cookies::{private_response, CookiePolicy};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_reading::{
    repositories::audience::PgAudience,
    services::audience::AudienceError,
};

#[derive(Clone)]
struct AudienceState {
    accounts: PgAccounts,
    cookies: CookiePolicy,
    store: PgAudience,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudienceQuery {
    days: Option<i32>,
}

impl AudienceQuery {
    fn days(&self) -> Result<i32, StatusCode> {
        match self.days.unwrap_or(7) {
            days @ (1 | 7 | 30 | 90) => Ok(days),
            _ => Err(StatusCode::BAD_REQUEST),
        }
    }
}

pub(crate) fn routes(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let store = PgAudience::new(accounts.pool().clone());
    Router::new()
        .route("/admin/audience", get(page))
        .with_state(AudienceState {
            accounts,
            cookies,
            store,
        })
}

fn secure(body: impl IntoResponse) -> Response {
    let mut response = private_response(body, &[]);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

async fn administrator(state: &AudienceState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let actor = state
        .accounts
        .session_actor(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !actor.administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

async fn page(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    query: Result<Query<AudienceQuery>, QueryRejection>,
) -> Response {
    // One deadline covers both fresh authorization and the aggregate read.
    let deadline = Instant::now() + Duration::from_millis(950);
    match timeout_at(deadline, administrator(&state, &headers)).await {
        Ok(Ok(())) => {}
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    // Carry extractor errors through authorization, and secure error responses too.
    let days = match query
        .map_err(|_| StatusCode::BAD_REQUEST)
        .and_then(|Query(query)| query.days())
    {
        Ok(days) => days,
        Err(status) => return secure(status),
    };
    match timeout_at(deadline, state.store.report(days)).await {
        Ok(Ok(report)) => secure(render::page(&report)),
        Ok(Err(AudienceError::Invalid)) => secure(StatusCode::BAD_REQUEST),
        Ok(Err(AudienceError::Unavailable)) | Err(_) => secure((
            StatusCode::SERVICE_UNAVAILABLE,
            render::unavailable(days),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header, Uri};

    #[test]
    fn only_bounded_periods_are_accepted() {
        assert_eq!(AudienceQuery::default().days(), Ok(7));
        for days in [1, 7, 30, 90] {
            let uri: Uri = format!("/admin/audience?days={days}").parse().unwrap();
            let query = Query::<AudienceQuery>::try_from_uri(&uri).unwrap();
            assert_eq!(query.days(), Ok(days));
        }
        for raw in ["days=0", "days=-1", "days=8", "days=91", "days=abc", "days=7&days=1", "reader=8"] {
            let uri: Uri = format!("/admin/audience?{raw}").parse().unwrap();
            let result = Query::<AudienceQuery>::try_from_uri(&uri)
                .map_err(|_| StatusCode::BAD_REQUEST)
                .and_then(|Query(query)| query.days());
            assert_eq!(result, Err(StatusCode::BAD_REQUEST), "{raw}");
        }
    }

    #[test]
    fn success_and_errors_are_private_and_not_indexable() {
        for status in [StatusCode::OK, StatusCode::BAD_REQUEST, StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN, StatusCode::SERVICE_UNAVAILABLE] {
            let response = secure(status);
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
            assert!(response.headers()["content-security-policy"].to_str().unwrap().contains("script-src 'none'"));
        }
    }
}
