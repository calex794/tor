//! Privacy controls and verified identity opt-in, owned by the account boundary.
use super::{deny, session_user, AuthState};
use crate::{
    auth_cookies::private_response,
    ril::{self, VerifiedRilSubject},
};
use axum::{
    body::Body,
    extract::{Form, MatchedPath, State},
    http::{header, HeaderMap, Method, Request, StatusCode},
    middleware::Next,
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::{ril_pseudonym::RilSubjectSigner, security::session_token_digest};
use tor_traffic_behavior::VerifiedSubjectKey;
use std::time::Duration;


#[derive(Deserialize)]
pub(super) struct BehaviorConsentForm {
    csrf_token: Option<String>,
    choice: String,
}

// Identified collection defaults OFF. Only a single-instance deployment with
// a separately provisioned HMAC key may enable opt-in behavior paths.
pub(super) fn configured_ril_signer() -> Option<RilSubjectSigner> {
    if std::env::var("TOR_RIL_SINGLE_INSTANCE").as_deref() != Ok("1") {
        return None;
    }
    std::env::var("TOR_RIL_HMAC_KEY")
        .ok()
        .and_then(|value| RilSubjectSigner::from_hex(&value))
}

/// Additive FLOW-02b coverage for opt-in accounts visiting public HTML pages.
/// Feature switch defaults OFF; A owns both session verification and consent.
#[derive(Clone)]
pub(crate) struct PublicFlowConsentContext {
    accounts: tor_accounts::repositories::PgAccounts,
    cookies: crate::auth_cookies::CookiePolicy,
    signer: Option<RilSubjectSigner>,
}

impl PublicFlowConsentContext {
    pub(crate) fn new(
        accounts: tor_accounts::repositories::PgAccounts,
        cookies: crate::auth_cookies::CookiePolicy,
    ) -> Self {
        let signer = if std::env::var("TOR_FLOW_PUBLIC_OPT_IN_ENABLED").as_deref() == Ok("1") {
            configured_ril_signer()
        } else {
            None
        };
        Self {
            accounts,
            cookies,
            signer,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.signer.is_some()
    }
}

fn eligible_public_flow_route(method: &Method, template: Option<&str>) -> bool {
    method == Method::GET
        && matches!(
            template,
            Some("/") | Some("/catalog") | Some("/books/{id}") | Some("/reader/{id}")
        )
}

/// The same A-owned signed HMAC + current consent gate is reused in both
/// authenticated and public flows. Never infer a user from a naked cookie,
/// IP, referrer or User-Agent; never propagate a session token to RIL.
async fn verified_opted_in_subject(
    accounts: &tor_accounts::repositories::PgAccounts,
    cookies: &crate::auth_cookies::CookiePolicy,
    signer: Option<&RilSubjectSigner>,
    headers: &HeaderMap,
    budget: Duration,
) -> Option<VerifiedSubjectKey> {
    let signer = signer?;
    let token = cookies.session_token(headers)?;
    let digest = session_token_digest(&token);
    let verified = tokio::time::timeout(
        budget,
        accounts.ril_verified_opted_in_session(&digest),
    )
    .await
    .ok()?
    .ok()??;
    Some(VerifiedSubjectKey::from_hmac_digest(
        signer.pseudonym(verified.0, verified.1),
    ))
}

/// Opt-in coverage only. It is attached to four public route registrations,
/// never as an outer middleware over all routes. On missing token/consent,
/// unavailable DB, or timeout the request proceeds normally as anonymous.
/// The 25ms budget caps the *additional* public-page latency; no new SQL
/// is issued on the anonymous request path.
pub(crate) async fn annotate_public_flow(
    State(context): State<PublicFlowConsentContext>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str);
    if !eligible_public_flow_route(request.method(), template) || !context.enabled() {
        return next.run(request).await;
    }
    let subject = verified_opted_in_subject(
        &context.accounts,
        &context.cookies,
        context.signer.as_ref(),
        request.headers(),
        Duration::from_millis(25),
    )
    .await;
    let mut response = next.run(request).await;
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let value = value.trim().to_ascii_lowercase();
            value == "text/html" || value.starts_with("text/html;")
        });
    if response.status() == StatusCode::OK && html {
        if let Some(subject) = subject {
            response.extensions_mut().insert(VerifiedRilSubject(subject));
        }
    }
    response
}

// The outer RIL layer cannot inspect cookies to invent a user. Only this
// A-owned layer looks up a current PG session and explicit consent.
pub(super) async fn annotate_verified_ril(
    State(state): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str);
    let observe = request.method() == Method::GET
        && matches!(
            template,
            Some("/me")
                | Some("/me/library")
                | Some("/read/{id}")
                | Some("/me/books")
                | Some("/me/books/new")
                | Some("/me/books/{id}")
        );
    let verified = if observe {
        verified_opted_in_subject(
            &state.accounts,
            &state.cookies,
            state.ril_signer.as_ref(),
            request.headers(),
            Duration::from_millis(100),
        )
        .await
    } else {
        None
    };
    let mut response = next.run(request).await;
    if response.status().is_success() {
        if let Some(key) = verified {
            response.extensions_mut().insert(VerifiedRilSubject(key));
        }
    }
    response
}

/// A separately enabled, CSRF-protected opt-in. Neither token nor user ID
/// comes from the form. A changed consent epoch makes old HMAC identities stale.
pub(super) async fn set_behavior_consent(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<BehaviorConsentForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let enable = match form.choice.as_str() {
        "enable" => true,
        "disable" => false,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    if enable && state.ril_signer.is_none() {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let epoch = match state.accounts.set_ril_behavior_consent(actor, enable).await {
        Ok(epoch) => epoch,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !enable && epoch > 1 {
        if let Some(signer) = state.ril_signer.as_ref() {
            let key = VerifiedSubjectKey::from_hmac_digest(signer.pseudonym(actor, epoch - 1));
            if !ril::forget_subject(key).await {
                // PG consent already disabled. Do not claim erased until
                // both in-process workers have acknowledged deletion.
                return deny(StatusCode::SERVICE_UNAVAILABLE);
            }
        }
    }
    private_response(Redirect::to("/me"), &[])
}

#[cfg(test)]
mod public_flow_tests {
    use super::*;

    #[test]
    fn only_four_public_html_routes_are_eligible() {
        for template in ["/", "/catalog", "/books/{id}", "/reader/{id}"] {
            assert!(eligible_public_flow_route(&Method::GET, Some(template)));
            assert!(!eligible_public_flow_route(&Method::POST, Some(template)));
            assert!(!eligible_public_flow_route(&Method::HEAD, Some(template)));
        }
        for template in [
            "/api/v1/books/{id}/toc",
            "/covers/{id}/{variant}",
            "/assets/site.css",
            "/ops/overview",
            "/me",
            "/read/{id}",
            "/books/{id}/stats",
            "/admin",
        ] {
            assert!(!eligible_public_flow_route(&Method::GET, Some(template)));
        }
        assert!(!eligible_public_flow_route(&Method::GET, None));
    }
}
