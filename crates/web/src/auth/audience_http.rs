//! FLOW: site-administrator access to reading, discovery and saved page views.
//! No tracking identifier, collection switch, SQL or operator-password fallback.
mod books;
mod groups;
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
use tor_reading::{repositories::audience::PgAudience, services::audience::AudienceError};
use tor_recommendations::{
    repositories::audience::PgDiscoveryAudience, services::audience::DiscoveryError,
};
use tor_traffic_behavior::{repositories::PgFlowAudience, services::audience::FlowHourlyError};

#[derive(Clone)]
struct AudienceState {
    accounts: PgAccounts,
    cookies: CookiePolicy,
    store: PgAudience,
    discovery: PgDiscoveryAudience,
    traffic: PgFlowAudience,
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
    let discovery = PgDiscoveryAudience::new(accounts.pool().clone());
    let traffic = PgFlowAudience::new(accounts.pool().clone());
    Router::new()
        .route("/admin/audience", get(page))
        .route("/admin/audience/books", get(books::page))
        .route("/admin/audience/groups", get(groups::page))
        .with_state(AudienceState {
            accounts,
            cookies,
            store,
            discovery,
            traffic,
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
    // One deadline covers fresh authorization and all independent aggregate reads.
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
    let hours_supported = matches!(days, 1 | 7);
    let (reading, sources, hours) = tokio::join!(
        timeout_at(deadline, state.store.report(days)),
        timeout_at(deadline, state.discovery.report(days)),
        async {
            if hours_supported {
                timeout_at(deadline, state.traffic.report(days)).await.ok()
            } else {
                None
            }
        },
    );
    if matches!(reading, Ok(Err(AudienceError::Invalid)))
        || matches!(sources, Ok(Err(DiscoveryError::Invalid)))
        || matches!(hours, Some(Err(FlowHourlyError::Invalid)))
    {
        return secure(StatusCode::BAD_REQUEST);
    }
    let reading = reading.ok().and_then(Result::ok);
    let sources = sources.ok().and_then(Result::ok);
    let hours = hours.and_then(Result::ok);
    let status = if reading.is_some() && sources.is_some() && (!hours_supported || hours.is_some())
    {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    secure((
        status,
        render::page(days, reading.as_ref(), sources.as_ref(), hours.as_ref()),
    ))
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
        for raw in [
            "days=0",
            "days=-1",
            "days=8",
            "days=91",
            "days=abc",
            "days=7&days=1",
            "reader=8",
        ] {
            let uri: Uri = format!("/admin/audience?{raw}").parse().unwrap();
            let result = Query::<AudienceQuery>::try_from_uri(&uri)
                .map_err(|_| StatusCode::BAD_REQUEST)
                .and_then(|Query(query)| query.days());
            assert_eq!(result, Err(StatusCode::BAD_REQUEST), "{raw}");
        }
    }

    #[test]
    fn success_and_errors_are_private_and_not_indexable() {
        for status in [
            StatusCode::OK,
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let response = secure(status);
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
            assert!(response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("script-src 'none'"));
        }
    }
}
