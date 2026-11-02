//! E-11 / S051: every admin read/poll/write revalidates a live account role.
use super::support::{service, status, MessageView, PageQuery, SendForm};
use super::{deny, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tor_accounts::services::security::session_token_digest;
use tor_contracts::auth::SessionActor;

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/admin/support", get(index))
        .route("/admin/support/{id}", get(page))
        .route("/admin/support/{id}/read", axum::routing::post(mark_read))
        .route("/admin/support/{id}/messages", get(messages).post(submit))
}

pub(super) async fn administrator(
    state: &AuthState,
    headers: &HeaderMap,
) -> Result<SessionActor, StatusCode> {
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
    Ok(actor)
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IndexQuery {
    before: Option<i64>,
}

pub(super) async fn index(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<IndexQuery>,
) -> Response {
    let actor = match administrator(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return denied(s),
    };
    let chats = match service(&state).admin_chats(actor, query.before, 100).await {
        Ok(c) => c,
        Err(e) => return denied(status(e)),
    };
    let ids: Vec<_> = chats.iter().filter_map(|chat| chat.requester_id).collect();
    let names: std::collections::HashMap<_, _> = match state.accounts.user_display_names(&ids).await
    {
        Ok(rows) => rows.into_iter().collect(),
        Err(_) => return denied(StatusCode::SERVICE_UNAVAILABLE),
    };
    let named: Vec<_> = chats
        .into_iter()
        .map(|chat| {
            let name = chat
                .requester_id
                .and_then(|id| names.get(&id).cloned())
                .unwrap_or_else(|| chat.guest_alias.clone());
            (chat, name)
        })
        .collect();
    super::support::html_response(super::support_pages::index(&named), &[])
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(query): Query<PageQuery>,
) -> Response {
    let actor = match administrator(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return denied(s),
    };
    let data = match service(&state)
        .admin_messages(actor, id, query.after_id, query.before_id, 100)
        .await
    {
        Ok(d) => d,
        Err(e) => return denied(status(e)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    super::support::html_response(super::support_pages::chat(&data, &csrf, true), &cookies)
}

pub(super) async fn messages(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(query): Query<PageQuery>,
) -> Response {
    let actor = match administrator(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return denied(s),
    };
    let data = match service(&state)
        .admin_messages(actor, id, query.after_id, query.before_id, 100)
        .await
    {
        Ok(d) => d,
        Err(e) => return denied(status(e)),
    };
    #[derive(Serialize)]
    struct Poll {
        ok: bool,
        messages: Vec<MessageView>,
        has_more: bool,
    }
    private_response(
        Json(Poll {
            ok: true,
            messages: data
                .messages
                .into_iter()
                .map(|m| MessageView::new(m, true))
                .collect(),
            has_more: data.has_more,
        })
        .into_response(),
        &[],
    )
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<SendForm>,
) -> Response {
    let actor = match administrator(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return denied(s),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return denied(StatusCode::FORBIDDEN);
    }
    let key = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .or(form.client_message_id.as_deref());
    match service(&state)
        .admin_send(
            actor,
            id,
            &form.body,
            key,
            &tor_accounts::services::security::generate_session_token(),
        )
        .await
    {
        Ok(message)
            if headers
                .get("accept")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.contains("application/json")) =>
        {
            #[derive(Serialize)]
            struct Receipt {
                ok: bool,
                message: MessageView,
            }
            private_response(
                Json(Receipt {
                    ok: true,
                    message: MessageView::new(message, true),
                })
                .into_response(),
                &[],
            )
        }
        Ok(_) => private_response(Redirect::to(&format!("/admin/support/{id}")), &[]),
        Err(tor_support::domain::SupportError::Invalid)
            if !headers
                .get("accept")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.contains("application/json")) =>
        {
            private_response(
                (
                    StatusCode::BAD_REQUEST,
                    super::support_pages::invalid_message(
                        &form.body,
                        &form.csrf_token,
                        &format!("/admin/support/{id}"),
                    ),
                ),
                &[],
            )
        }
        Err(e) => super::support::json_error(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadForm {
    csrf_token: String,
    message_id: i64,
}
pub(super) async fn mark_read(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<ReadForm>,
) -> Response {
    let actor = match administrator(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return denied(s),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return denied(StatusCode::FORBIDDEN);
    }
    match service(&state)
        .admin_mark_read(
            actor,
            id,
            form.message_id,
            &tor_accounts::services::security::generate_session_token(),
        )
        .await
    {
        Ok(()) => private_response(Redirect::to(&format!("/admin/support/{id}")), &[]),
        Err(e) => denied(status(e)),
    }
}

fn denied(status: StatusCode) -> Response {
    tracing::warn!(
        status = status.as_u16(),
        request_id = tor_accounts::services::security::generate_session_token(),
        "support administrative request refused"
    );
    deny(status)
}
