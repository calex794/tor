//! AUTH-04b: exact account lookup and password-confirmed session revocation.
use super::super::AuthState;
use crate::{
    auth_cookies::{deny, private_response},
    pages,
};
use axum::{
    extract::{DefaultBodyLimit, Form, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::{
    repositories::admin_sessions::{AdminSessionOverview, AdminSessionRevokeError},
    services::security::{session_token_digest, verify_password_for_step_up},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lookup {
    user_id: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeForm {
    user_id: i64,
    session_id: String,
    current_password: String,
    csrf_token: String,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route(
        "/odmen/security/sessions",
        get(page).post(revoke).layer(DefaultBodyLimit::max(4096)),
    )
}

async fn operator(
    state: &AuthState,
    headers: &HeaderMap,
) -> Result<([u8; 32], i64), StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let actor = state
        .accounts
        .session_actor(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !actor.administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok((digest, actor.actor.user_id.get()))
}

fn shell(body: &str, cookies: &[String]) -> Response {
    let mut response = private_response(
        pages::layout(
            "Сеансы пользователей",
            "Управление сеансами через действующую роль администратора.",
            &format!(
                r#"<section class="section"><div class="container">
                <h1>Сеансы пользователей</h1>{body}
                <p><a href="/odmen">Администрирование</a></p>
                </div></section>"#
            ),
        ),
        cookies,
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

fn form(target: i64, session_id: &str, csrf: &str, label: &str) -> String {
    format!(
        r#"<form method="post" action="/odmen/security/sessions">
        <input type="hidden" name="user_id" value="{target}">
        <input type="hidden" name="session_id" value="{}">
        <input type="hidden" name="csrf_token" value="{}">
        <label>Пароль администратора
        <input type="password" name="current_password" autocomplete="current-password"
               maxlength="512" required></label>
        <button type="submit">{}</button></form>"#,
        pages::escape_html(session_id),
        pages::escape_html(csrf),
        pages::escape_html(label),
    )
}

fn render_target(target: i64, overview: &AdminSessionOverview, csrf: &str) -> String {
    let mut body = format!(
        "<h2>{} · ID {target}</h2><p>Состояние: {}</p>",
        pages::escape_html(&overview.display_name),
        pages::escape_html(&overview.account_state),
    );
    if overview.sessions.is_empty() {
        body.push_str("<p>Действующих сеансов нет.</p>");
        return body;
    }
    body.push_str("<p>Показано до 50 сеансов. Массовый отзыв охватывает все устройства. Новый вход остаётся возможен, если аккаунт не заблокирован.</p>");
    body.push_str(&form(target, "all", csrf, "Завершить все сеансы пользователя"));
    for session in &overview.sessions {
        let _ = write!(
            body,
            "<article class=\"me-now-card\"><h3>Браузер: {}</h3>
             <p>Вход (UTC): {} · Действует до (UTC): {}</p>{}</article>",
            pages::escape_html(&session.browser_family),
            pages::escape_html(&session.opened_utc),
            pages::escape_html(&session.expires_utc),
            form(target, &session.id, csrf, "Завершить этот сеанс"),
        );
    }
    body
}

async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<Lookup>,
) -> Response {
    let (digest, actor) = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    let mut body = String::from(
        r#"<form method="get" action="/odmen/security/sessions">
        <label>ID пользователя
        <input type="number" name="user_id" min="1" required></label>
        <button type="submit">Найти сеансы</button></form>
        <p>Для собственных устройств используйте
        <a href="/me/security/sessions">настройки аккаунта</a>.</p>"#,
    );
    let Some(target) = query.user_id else {
        return shell(&body, &[]);
    };
    if target <= 0 || target == actor {
        return deny(StatusCode::BAD_REQUEST);
    }
    let overview = match state
        .accounts
        .administrator_session_overview(&digest, target)
        .await
    {
        Ok(Some(overview)) => overview,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    body.push_str(&render_target(target, &overview, &csrf));
    shell(&body, &cookies)
}

async fn revoke(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<RevokeForm>,
) -> Response {
    let (digest, actor) = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    if form.user_id <= 0
        || form.user_id == actor
        || form.current_password.is_empty()
        || form.current_password.len() > 512
        || form.session_id.len() > 36
    {
        return deny(StatusCode::BAD_REQUEST);
    }
    let key = session_token_digest(&format!("auth:admin-revoke-session:{actor}"));
    match state.accounts.permit_auth_attempt(&key, 8, 600).await {
        Ok(true) => {}
        Ok(false) => return deny(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let hash = match state.accounts.password_hash_for_user(actor).await {
        Ok(Some(hash)) => hash,
        _ => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let password = form.current_password;
    let proof = match state
        .password_jobs
        .run(move || verify_password_for_step_up(&password, &hash))
        .await
    {
        Ok(Some(proof)) => proof,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let selected = (form.session_id != "all").then_some(form.session_id.as_str());
    match state
        .accounts
        .revoke_administrator_target_sessions(&digest, form.user_id, selected, &proof)
        .await
    {
        Ok(count) => shell(
            &format!(
                r#"<p>Завершено сеансов: {count}.</p>
                <p><a href="/odmen/security/sessions?user_id={}">Проверить сеансы пользователя</a></p>"#,
                form.user_id
            ),
            &[],
        ),
        Err(AdminSessionRevokeError::Denied) => deny(StatusCode::FORBIDDEN),
        Err(AdminSessionRevokeError::InvalidTarget) => deny(StatusCode::BAD_REQUEST),
        Err(AdminSessionRevokeError::StepUpRequired) => deny(StatusCode::UNAUTHORIZED),
        Err(AdminSessionRevokeError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
