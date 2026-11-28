//! Admit both login aliases through one private account budget.
use super::{auth_limit_error, login_storage_error, spray_rate_error};
use crate::{
    auth::{auth_observation, login_limiter, AuthState},
    observability,
};
use axum::{http::StatusCode, response::Response};
use tor_accounts::{
    domain::access_audit::LoginAuditContext, repositories::postgres::StoredUser,
    services::login_identity::LoginIdentity,
};
use tor_observability::services::auth::LoginOutcome;

pub(super) struct Admission {
    pub(super) stored: Option<StoredUser>,
    pub(super) spray: Option<login_limiter::SprayGuard>,
    pub(super) account_allowed: bool,
}

pub(super) fn rate_error(
    status: StatusCode,
    observed: &mut auth_observation::RequestObservation,
) -> Response {
    observed.report(
        if status == StatusCode::TOO_MANY_REQUESTS {
            auth_observation::Outcome::RateLimited
        } else {
            auth_observation::Outcome::Unavailable
        },
        None,
    );
    auth_limit_error(status, false)
}

pub(super) async fn admit(
    state: &AuthState,
    identity: &LoginIdentity,
    audit: LoginAuditContext,
    observed: &mut auth_observation::RequestObservation,
) -> Result<Admission, Box<Response>> {
    // The route already charged IP and submitted identifier before lookup.
    let source = match login_limiter::spray_start(state, audit.verified_ip).await {
        Ok(source) => source,
        Err(login_limiter::SprayError::Locked(seconds)) => {
            observed.report(auth_observation::Outcome::RateLimited, None);
            observability::record_login_outcome(LoginOutcome::RateLimited);
            return Err(Box::new(spray_rate_error(seconds)));
        }
        Err(login_limiter::SprayError::Unavailable) => {
            eprintln!("TOR_AUTH_UNAVAILABLE stage=spray_preflight");
            return Err(Box::new(rate_error(
                StatusCode::SERVICE_UNAVAILABLE,
                observed,
            )));
        }
    };
    let stored = state
        .accounts
        .find_user_by_login(identity)
        .await
        .map_err(|_| {
            observed.report(auth_observation::Outcome::Unavailable, None);
            Box::new(login_storage_error(LoginOutcome::LookupUnavailable))
        })?;
    let canonical = stored
        .as_ref()
        .map_or(identity.as_str(), |user| user.email_normalized.as_str());
    observed.set_identity(canonical);

    // Keep private budget exhaustion out of the public 429 responses.
    let account_allowed = state
        .accounts
        .permit_login_account_attempt(stored.as_ref().map(|user| user.id), identity)
        .await
        .map_err(|_| {
            eprintln!("TOR_AUTH_UNAVAILABLE stage=account_rate_limit");
            Box::new(rate_error(StatusCode::SERVICE_UNAVAILABLE, observed))
        })?;
    // The observable spray threshold counts submitted identities, not the
    // private relationship between a mailbox and a public handle.
    let spray = source.map(|source| source.for_target(identity.as_str()));
    Ok(Admission {
        stored,
        spray,
        account_allowed,
    })
}
