//! FLOW captures only successful, consented HTML navigation. No body is read.
use crate::auth_cookies::CookiePolicy;
use axum::{
    extract::{MatchedPath, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_recommendations::{
    repositories::{visits::PgFlowVisits, PgRecommendations},
    services::visits::VisitObservation,
};

mod queue;
pub(crate) use queue::snapshot;

#[derive(Clone)]
pub(crate) struct Context {
    accounts: PgAccounts,
    cookies: CookiePolicy,
    recommendations: PgRecommendations,
}

impl Context {
    pub(crate) fn new(accounts: PgAccounts, cookies: CookiePolicy) -> Self {
        let recommendations = PgRecommendations::new(accounts.pool().clone());
        Self {
            accounts,
            cookies,
            recommendations,
        }
    }
}

#[derive(Clone)]
struct Capture {
    context: Context,
    queue: queue::Queue,
}

pub(crate) fn instrument(app: Router, context: Context) -> Router {
    if !collection_enabled() {
        return app;
    }
    let Some(queue) = queue::Queue::start(PgFlowVisits::new(context.accounts.pool().clone()))
    else {
        return app;
    };
    app.layer(middleware::from_fn_with_state(
        Capture { context, queue },
        observe,
    ))
}

pub(crate) fn collection_enabled() -> bool {
    [
        "TOR_FLOW_VISITS_ENABLED",
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
    ]
    .iter()
    .all(|key| std::env::var(key).as_deref() == Ok("1"))
}

fn surface(method: &Method, template: Option<&str>) -> Option<i16> {
    if method != Method::GET {
        return None;
    }
    match template? {
        "/" => Some(0),
        "/catalog" => Some(1),
        "/books/{id}" => Some(2),
        "/reader/{id}" | "/read/{id}" | "/read/paid/{chapter_id}" | "/read/layer/{chapter_id}" => {
            Some(3)
        }
        "/me/library" | "/me/purchases" => Some(4),
        _ => None,
    }
}

fn html_success(response: &Response) -> bool {
    response.status() == StatusCode::OK
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("text/html"))
            })
}

async fn scope(
    context: &Context,
    headers: &HeaderMap,
) -> Result<Option<tor_recommendations::services::visits::FlowScope>, ()> {
    let Some(token) = context.cookies.session_token(headers) else {
        return Ok(None);
    };
    let digest = session_token_digest(&token);
    let Some(actor) = context
        .accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| ())?
    else {
        return Ok(None);
    };
    context
        .recommendations
        .flow_scope(actor)
        .await
        .map_err(|_| ())
}

async fn observe(State(state): State<Capture>, request: Request, next: Next) -> Response {
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str);
    let Some(surface) = surface(request.method(), template) else {
        return next.run(request).await;
    };
    if crate::ril::flow::likely_bot(request.headers()) {
        return next.run(request).await;
    }
    // Never wait for a queue/SQL write on the HTTP path. Session verification
    // plus opt-in lookup together have a 25ms optional observation budget.
    let scope = tokio::time::timeout(
        Duration::from_millis(25),
        scope(&state.context, request.headers()),
    )
    .await;
    let scope = match scope {
        Ok(Ok(value)) => value,
        Ok(Err(())) | Err(_) => {
            state.queue.scope_timeout();
            None
        }
    };
    let source = match crate::ril::flow::referrer_hint(request.headers()) {
        tor_contracts::flow::AcquisitionChannel::ExternalOrganicSearch => 0,
        tor_contracts::flow::AcquisitionChannel::ExternalReferral => 1,
        tor_contracts::flow::AcquisitionChannel::ExternalPaid => 2,
        tor_contracts::flow::AcquisitionChannel::DirectOrUnknown => 3,
    };
    let response = next.run(request).await;
    if html_success(&response) {
        if let Some(scope) = scope {
            if let Ok(elapsed) = SystemTime::now().duration_since(UNIX_EPOCH) {
                if let Ok(millis) = i64::try_from(elapsed.as_millis()) {
                    if let Ok(observation) = VisitObservation::new(scope, millis, surface, source) {
                        state.queue.offer(observation);
                    }
                }
            }
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_reading_navigation_is_collected() {
        for template in [
            "/",
            "/catalog",
            "/books/{id}",
            "/reader/{id}",
            "/read/{id}",
            "/read/paid/{chapter_id}",
            "/read/layer/{chapter_id}",
            "/me/library",
            "/me/purchases",
        ] {
            assert!(surface(&Method::GET, Some(template)).is_some());
            assert!(surface(&Method::POST, Some(template)).is_none());
            assert!(surface(&Method::HEAD, Some(template)).is_none());
        }
        for template in [
            "/read/admin/{chapter_id}",
            "/mail",
            "/admin/audience",
            "/me/books/{id}",
            "/healthz",
            "/assets/site.css",
            "/api/v1/reader/chapters/{chapter_id}/grant",
        ] {
            assert_eq!(surface(&Method::GET, Some(template)), None);
        }
    }
}
