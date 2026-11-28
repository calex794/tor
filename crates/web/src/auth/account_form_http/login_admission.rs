//! Admit both login aliases through the existing canonical account budget.
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
    pub(super) identity_key: [u8; 32],
    pub(super) submitted_key: [u8; 32],
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
    submitted_key: [u8; 32],
    audit: LoginAuditContext,
    observed: &mut auth_observation::RequestObservation,
) -> Result<Admission, Response> {
    // The route has already charged the IP and submitted identifier.
    // Preserve the source spray fence before resolving the account.
    let source = match login_limiter::spray_start(state, audit.verified_ip).await {
        Ok(source) => source,
        Err(login_limiter::SprayError::Locked(seconds)) => {
            observed.report(auth_observation::Outcome::RateLimited, None);
            observability::record_login_outcome(LoginOutcome::RateLimited);
            return Err(spray_rate_error(seconds));
        }
        Err(login_limiter::SprayError::Unavailable) => {
            eprintln!("TOR_AUTH_UNAVAILABLE stage=spray_preflight");
            return Err(rate_error(StatusCode::SERVICE_UNAVAILABLE, observed));
        }
    };
    let stored = state
        .accounts
        .find_user_by_login(identity)
        .await
        .map_err(|_| {
            observed.report(auth_observation::Outcome::Unavailable, None);
            login_storage_error(LoginOutcome::LookupUnavailable)
        })?;
    let canonical = stored
        .as_ref()
        .map_or(identity.as_str(), |user| user.email_normalized.as_str());
    observed.set_identity(canonical);

    // Every resolved account uses the pre-existing email budget. The input
    // budget is only an additional lookup guard, not a second password budget.
    // Unknown identities retain the original one-bucket behavior.
    let identity_key = if canonical == identity.as_str() {
        submitted_key
    } else {
        login_limiter::limit_identity(state, "login", canonical)
            .await
            .map_err(|status| rate_error(status, observed))?
    };
    let spray = source.map(|source| source.for_target(canonical));
    Ok(Admission {
        stored,
        spray,
        identity_key,
        submitted_key,
    })
}

pub(super) async fn clear(
    state: &AuthState,
    identity_key: [u8; 32],
    submitted_key: [u8; 32],
) -> Result<(), StatusCode> {
    if identity_key != submitted_key {
        state
            .accounts
            .clear_auth_attempt(&submitted_key)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    }
    state
        .accounts
        .clear_auth_attempt(&identity_key)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(())
}
