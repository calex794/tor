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
        MailError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
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
    peer_id: i64,
    csrf_token: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SendForm {
    body: String,
    command_key: String,
    csrf_token: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadForm {
    through_id: i64,
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
    let limit = query.limit.unwrap_or(25);
    let conversations = match service(&state).mailbox(actor, query.after_id, limit).await {
        Ok(p) => p,
        Err(e) => return deny(status(e)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        super::mail_pages::index(actor, &conversations, limit, &csrf),
        &cookies,
    )
}

pub(super) async fn start(
    State(state): State<AuthState>,
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
    match service(&state).start(actor, form.peer_id).await {
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
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let key = generate_session_token();
    let mut response = private_response(
        super::mail_pages::conversation(actor, id, &page, &csrf, &key),
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
    match service(&state)
        .send(actor, id, &form.body, &form.command_key)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/mail/{id}")), &[]),
        Err(e) => deny(status(e)),
    }
}

pub(super) async fn read(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ReadForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return deny(s),
    };
    if let Err(s) = csrf(&state, &headers, form.csrf_token.as_deref()) {
        return deny(s);
    }
    match service(&state).mark_read(actor, id, form.through_id).await {
        Ok(()) => private_response(Redirect::to(&format!("/mail/{id}")), &[]),
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
