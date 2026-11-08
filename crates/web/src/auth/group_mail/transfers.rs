//! Group management is offered, not assigned without the successor's consent.
use super::super::{deny, session_user, AuthState};
use super::{group_store, secure, shell};
use crate::{auth_cookies::private_response, pages::escape_html};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;

pub(crate) fn router() -> Router<AuthState> {
    Router::new()
        .route("/mail/group-transfers", get(inbox))
        .route("/mail/group-transfers/{id}/accept", post(accept))
        .route("/mail/group-transfers/{id}/decline", post(decline))
        .route("/mail/groups/{id}/transfer/{offer_id}/revoke", post(revoke))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionForm {
    csrf_token: String,
}

async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    session_user(state, headers)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn csrf(state: &AuthState, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

async fn inbox(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let store = group_store(&state);
    let incoming = match store.pending_transfer_incoming(actor).await {
        Ok(offers) => offers,
        Err(error) => return deny(super::super::mail::status(error)),
    };
    let outgoing = match store.pending_transfer_outgoing(actor).await {
        Ok(offers) => offers,
        Err(error) => return deny(super::super::mail::status(error)),
    };
    let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
    let csrf_token = escape_html(&csrf_token);
    let mut body = String::from(
        r#"<header class="ui-page-header"><h1>Передача управления группами</h1><a href="/mail/groups">К группам</a></header><p>Роль владельца меняется только после вашего принятия. Предложение действует семь дней. Получив управление, вы сможете передать его дальше, но не сможете выйти из группы до передачи или её разрешённого удаления.</p><h2>Предложения вам</h2>"#,
    );
    if incoming.is_empty() {
        body.push_str("<p>Нет ожидающих предложений.</p>");
    }
    for offer in &incoming {
        write!(
            body,
            r#"<article class="me-now-card"><h3>{}</h3><p>От: {}</p><p><a href="/mail/groups/{}">Посмотреть группу</a></p><form method="post" action="/mail/group-transfers/{}/accept"><input type="hidden" name="csrf_token" value="{}"><button type="submit">Принять управление</button></form><form method="post" action="/mail/group-transfers/{}/decline"><input type="hidden" name="csrf_token" value="{}"><button type="submit">Отказаться</button></form></article>"#,
            escape_html(&offer.title),
            escape_html(&offer.other_name),
            offer.group_id,
            offer.id,
            csrf_token,
            offer.id,
            csrf_token,
        )
        .expect("incoming creator offer");
    }
    body.push_str("<h2>Ваши предложения</h2>");
    if outgoing.is_empty() {
        body.push_str("<p>Нет ожидающих ответов.</p>");
    }
    for offer in &outgoing {
        write!(
            body,
            r#"<article class="me-now-card"><h3>{}</h3><p>Предложено: {}</p><form method="post" action="/mail/groups/{}/transfer/{}/revoke"><input type="hidden" name="csrf_token" value="{}"><button type="submit">Отозвать предложение</button></form></article>"#,
            escape_html(&offer.title),
            escape_html(&offer.other_name),
            offer.group_id,
            offer.id,
            csrf_token,
        )
        .expect("outgoing creator offer");
    }
    let mut response = private_response(shell(&body), &cookies);
    secure(&mut response, false);
    response
}

async fn accept(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state).accept_transfer(actor, id).await {
        Ok(group_id) => private_response(Redirect::to(&format!("/mail/groups/{group_id}")), &[]),
        Err(error) => deny(super::super::mail::status(error)),
    }
}

async fn decline(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state).decline_transfer(actor, id).await {
        Ok(()) => private_response(Redirect::to("/mail/group-transfers"), &[]),
        Err(error) => deny(super::super::mail::status(error)),
    }
}

async fn revoke(
    State(state): State<AuthState>,
    Path((id, offer_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state)
        .revoke_transfer(actor, id, offer_id)
        .await
    {
        Ok(()) => private_response(Redirect::to("/mail/group-transfers"), &[]),
        Err(error) => deny(super::super::mail::status(error)),
    }
}
