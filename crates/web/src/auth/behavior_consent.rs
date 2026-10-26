//! Privacy controls and verified identity opt-in, owned by the account boundary.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, ril::{self, VerifiedRilSubject}};
use axum::{
    body::Body,
    extract::{Form, MatchedPath, State},
    http::{HeaderMap, Method, Request, StatusCode},
    middleware::Next,
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::{ril_pseudonym::RilSubjectSigner, security::session_token_digest};
use tor_traffic_behavior::VerifiedSubjectKey;

#[derive(Deserialize)]
struct BehaviorConsentForm {
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

// The outer RIL layer cannot inspect cookies to invent a user. Only this
// A-owned layer looks up a current PG session and explicit consent.
pub(super) async fn annotate_verified_ril(
    State(state): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let template = request.extensions().get::<MatchedPath>().map(MatchedPath::as_str);
    let observe = request.method() == Method::GET
        && matches!(template, Some("/me") | Some("/me/library") | Some("/read/{id}")
            | Some("/me/books") | Some("/me/books/new") | Some("/me/books/{id}"));
    let verified = if observe {
        if let (Some(signer), Some(token)) = (
            state.ril_signer.as_ref(),
            state.cookies.session_token(request.headers()),
        ) {
            let digest = session_token_digest(&token);
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                state.accounts.ril_verified_opted_in_session(&digest),
            ).await {
                Ok(Ok(Some((id, epoch)))) => Some(VerifiedSubjectKey::from_hmac_digest(
                    signer.pseudonym(id, epoch),
                )),
                _ => None,
            }
        } else {
            None
        }
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
        || !state.cookies.valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
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
