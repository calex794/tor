//! S052: immutable alias creation, never sponsor reassignment or payout.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_referrals::repositories::postgres::AliasClaim;

#[derive(Deserialize)]
pub(super) struct ClaimAliasForm {
    csrf_token: Option<String>,
    prefix: String,
    alias: String,
}

fn status_for_claim(claim: AliasClaim) -> StatusCode {
    match claim {
        AliasClaim::Created | AliasClaim::Unchanged => StatusCode::NO_CONTENT,
        AliasClaim::Invalid => StatusCode::BAD_REQUEST,
        AliasClaim::Taken | AliasClaim::Locked => StatusCode::CONFLICT,
        AliasClaim::Unavailable => StatusCode::FORBIDDEN,
    }
}

pub(super) async fn browser_create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ClaimAliasForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let redirect = match state
        .referrals
        .claim_reader_alias(actor, &form.prefix, &form.alias)
        .await
    {
        Ok(AliasClaim::Created | AliasClaim::Unchanged) => "/me/referrals",
        Ok(AliasClaim::Invalid) => "/me/referrals?alias_error=invalid",
        Ok(AliasClaim::Taken) => "/me/referrals?alias_error=taken",
        Ok(AliasClaim::Locked) => "/me/referrals?alias_error=locked",
        Ok(AliasClaim::Unavailable) => "/me/referrals?alias_error=unavailable",
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    private_response(Redirect::to(redirect), &[])
}

pub(super) async fn api_create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ClaimAliasForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state
        .referrals
        .claim_reader_alias(actor, &form.prefix, &form.alias)
        .await
    {
        Ok(decision) => deny(status_for_claim(decision)),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_09_alias_claim_maps_conflicts_without_side_effects() {
        assert_eq!(
            status_for_claim(AliasClaim::Invalid),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(status_for_claim(AliasClaim::Taken), StatusCode::CONFLICT);
        assert_eq!(status_for_claim(AliasClaim::Locked), StatusCode::CONFLICT);
        assert_eq!(
            status_for_claim(AliasClaim::Unavailable),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_for_claim(AliasClaim::Unchanged),
            StatusCode::NO_CONTENT
        );
    }
}
