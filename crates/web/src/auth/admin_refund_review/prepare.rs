//! S036: only a site administrator may prepare a support-linked refund.
//! This operation writes an intent but never credits funds.
use super::super::{deny, session_user, AuthState};
use super::{approval, response_error};
use crate::auth_cookies::private_response;
use axum::{
    extract::{rejection::FormRejection, Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::session_token_digest;
use tor_commerce::repositories::admin_support_refunds::PgSupportRefunds;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PrepareForm {
    buyer_id: String,
    purchase_id: String,
    conversation_id: String,
    command_key: String,
    reason: String,
    csrf_token: String,
}

fn positive(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 19 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok().filter(|v| *v > 0)
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    submitted: Result<Form<PrepareForm>, FormRejection>,
) -> Response {
    if !approval::enabled(&state) {
        return deny(StatusCode::NOT_FOUND);
    }
    if !matches!(session_user(&state, &headers).await, Ok(Some(_))) {
        return deny(StatusCode::UNAUTHORIZED);
    }
    let Form(form) = match submitted {
        Ok(form) => form,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Some(buyer), Some(purchase), Some(conversation)) = (
        positive(&form.buyer_id),
        positive(&form.purchase_id),
        positive(&form.conversation_id),
    ) else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    let repo = PgSupportRefunds::new(state.accounts.pool().clone());
    match repo
        .prepare_once(
            &digest,
            buyer,
            purchase,
            conversation,
            &form.command_key,
            &form.reason,
        )
        .await
    {
        Ok(_) => private_response(
            Redirect::to(&format!("/admin/finance/refunds/user/{buyer}")),
            &[],
        ),
        Err(err) => response_error(err),
    }
}
