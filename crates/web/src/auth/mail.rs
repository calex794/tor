//! E-08 browser adapter: verified actor, Origin/CSRF and feature service only.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::generate_session_token;
use tor_social::{domain::MailError, repositories::mail::PgMail, services::MailService};

pub(super) fn service(state: &AuthState) -> MailService<PgMail> {
    MailService::new(PgMail::new(state.accounts.pool().clone()))
}

pub(super) fn status(error: MailError) -> StatusCode {
    match error {
        MailError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        MailError::NotFound => StatusCode::NOT_FOUND,
        MailError::Conflict => StatusCode::CONFLICT,
        MailError::Blocked => StatusCode::FORBIDDEN,
        MailError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    session_user(state, headers)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn csrf(state: &AuthState, headers: &HeaderMap, token: Option<&str>) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers)
        || !token.is_some_and(|t| state.cookies.valid_csrf(headers, t))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MailQuery {
    pub after_id: Option<i64>,
    pub before_id: Option<i64>,
    pub limit: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartForm {
    csrf_token: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SendForm {
    body: String,
    command_key: Option<String>,
    client_message_id: Option<String>,
    csrf_token: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BlockForm {
    blocked: bool,
    csrf_token: Option<String>,
}

pub(super) async fn index(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<MailQuery>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(a) => a,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(s) => return deny(s),
    };
    if query.before_id.is_some() {
        return deny(StatusCode::BAD_REQUEST);
    }
    let limit = query.limit.unwrap_or(100);
    let conversations = match service(&state).mailbox(actor, query.after_id, limit).await {
        Ok(p) => p,
        Err(e) => return deny(status(e)),
    };
    // Only hydrate peers from the actor-scoped mailbox. Never expose email or
    // private account rows as a substitute for the source display name.
    let mut named = Vec::with_capacity(conversations.len());
    for conversation in conversations {
        let name = match state.accounts.user_display_name(conversation.peer_id).await {
            Ok(Some(name)) => name,
            Ok(None) => continue,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        };
        named.push((conversation, name));
    }
    private_response(super::mail_pages::index(&named, limit), &[])
}

pub(super) async fn start(
    State(state): State<AuthState>,
    Path(peer_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<StartForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return deny(s),
    };
    if let Err(s) = csrf(&state, &headers, form.csrf_token.as_deref()) {
        return deny(s);
    }
    match service(&state).start(actor, peer_id).await {
        Ok(id) => private_response(Redirect::to(&format!("/mail/{id}")), &[]),
        Err(e) => deny(status(e)),
    }
}

pub(super) async fn conversation(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<MailQuery>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(a) => a,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(s) => return deny(s),
    };
    let page = match service(&state)
        .messages(
            actor,
            id,
            query.after_id,
            query.before_id,
            query.limit.unwrap_or(50),
        )
        .await
    {
        Ok(p) => p,
        Err(e) => return deny(status(e)),
    };
    let peer_name = match state.accounts.user_display_name(page.peer_id).await {
        Ok(Some(name)) => name,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let friends = tor_social::repositories::friendships::PgFriendships::new(state.accounts.pool().clone());
    let friend_state = match friends.state(actor, page.peer_id).await {
        Ok(value) => value,
        Err(e) => return deny(status(e)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        super::mail_pages::conversation(actor, id, &page, &peer_name, &csrf, friend_state),
        &cookies,
    );
    response.headers_mut().insert("content-security-policy", "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'".parse().expect("static CSP"));
    response
}

pub(super) async fn send(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<SendForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return deny(s),
    };
    if let Err(s) = csrf(&state, &headers, form.csrf_token.as_deref()) {
        return deny(s);
    }
    let is_ajax = headers.get("x-requested-with").and_then(|x|x.to_str().ok()).is_some_and(|x|x.eq_ignore_ascii_case("fetch")||x.eq_ignore_ascii_case("xmlhttprequest"))
        || headers.get("accept").and_then(|x|x.to_str().ok()).is_some_and(|x|x.contains("application/json"));
    // NEwBOO reads the first nonempty sanitized identity from headers then form.
    // It never reinterprets a source HTML ID as a TOR JSON 43-character key.
    let client_id = [
        headers.get("idempotency-key").and_then(|h| h.to_str().ok()),
        headers.get("x-idempotency-key").and_then(|h| h.to_str().ok()),
        form.client_message_id.as_deref(),
    ].into_iter().flatten()
        .map(tor_social::domain::clean_html_client_id)
        .find(|id| !id.is_empty());
    let mail = service(&state);
    let result = if let Some(client_id) = client_id.as_deref() {
        mail.send_html(actor, id, &form.body, client_id, &generate_session_token()).await
    } else { match form.command_key.as_deref().filter(|key| !key.is_empty()) {
        Some(key) => mail.send(actor, id, &form.body, key).await,
        None => {
            mail.send_unkeyed(actor, id, &form.body, &generate_session_token())
                .await
        }
    }};
    match result {
        Ok(sent) if is_ajax => {
            // Retrieve the committed canonical row, including timestamp and replay identity.
            let page = match mail.messages(actor, id, Some(sent.id - 1), None, 1).await {
                Ok(page) => page,
                Err(e) => return deny(status(e)),
            };
            let Some(message) = page.items.into_iter().find(|m| m.id == sent.id) else {
                return deny(StatusCode::SERVICE_UNAVAILABLE);
            };
            let payload = serde_json::json!({"ok":true,"message":{
                "id": message.id.to_string(), "sender_id":message.sender_id.to_string(),
                "text":message.body, "created_at":message.created_at,
                "created_at_display":message.created_at_display
            }});
            private_response(axum::Json(payload), &[])
        },
        Ok(_) => private_response(Redirect::to(&format!("/mail/{id}")), &[]),
        Err(e) => deny(status(e)),
    }
}

pub(super) async fn block(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<BlockForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return deny(s),
    };
    if let Err(s) = csrf(&state, &headers, form.csrf_token.as_deref()) {
        return deny(s);
    }
    let page = match service(&state).messages(actor, id, None, None, 1).await {
        Ok(p) => p,
        Err(e) => return deny(status(e)),
    };
    match service(&state)
        .set_block(actor, page.peer_id, form.blocked)
        .await
    {
        Ok(()) => private_response(Redirect::to(&format!("/mail/{id}")), &[]),
        Err(e) => deny(status(e)),
    }
}

pub(super) async fn script() -> Response {
    (
        [
            ("content-type", "text/javascript; charset=utf-8"),
            ("x-content-type-options", "nosniff"),
        ],
        include_str!("../../assets/mail.js"),
    )
        .into_response()
}
