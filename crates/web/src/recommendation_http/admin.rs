//! Retired analytics controls still require the original live administrator and CSRF gates.
use super::{api, pages, Runtime};
use crate::{auth_cookies::deny, pages as html};
use axum::{
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_recommendations::{domain::feedback::FeedbackError, services::feedback::validate_revision};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QualityQuery {
    days: Option<i32>,
    channel: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Refresh,
    Activate,
    Hold,
    Rollback,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Control {
    action: Action,
    generation: Option<String>,
    expected_revision: i64,
    percent: Option<i16>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlForm {
    action: Action,
    generation: Option<String>,
    expected_revision: i64,
    percent: Option<i16>,
    csrf_token: String,
}
async fn actor(state: &Runtime, headers: &HeaderMap) -> Result<i64, StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let verified = state
        .accounts
        .session_actor(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !verified.administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(verified.actor.user_id.get())
}
fn csrf(state: &Runtime, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
pub(super) async fn quality(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Query(query): Query<QualityQuery>,
) -> Response {
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    let max_days = match query.channel.as_deref() {
        None | Some("recommendation") => 30,
        Some("organic") => 180,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    if !(1..=max_days).contains(&query.days.unwrap_or(7)) {
        return api::error(FeedbackError::Invalid);
    }
    api::retired()
}

fn validate_control(body: &Control) -> Result<(), FeedbackError> {
    validate_revision(body.expected_revision)?;
    if body
        .percent
        .is_some_and(|value| ![0, 1, 5, 10, 25, 50, 100].contains(&value))
    {
        return Err(FeedbackError::Invalid);
    }
    if let Some(generation) = &body.generation {
        if generation.len() != 36
            || !generation.bytes().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
        {
            return Err(FeedbackError::Invalid);
        }
    }
    if matches!(&body.action, Action::Activate) && body.generation.is_none() {
        return Err(FeedbackError::Invalid);
    }
    Ok(())
}

pub(super) async fn control(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Json(body): Json<Control>,
) -> Response {
    if let Err(status) = csrf(
        &state,
        &headers,
        headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    ) {
        return deny(status);
    }
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    if let Err(error) = validate_control(&body) {
        return api::error(error);
    }
    api::retired()
}

pub(super) async fn form_control(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(body): Form<ControlForm>,
) -> Response {
    if let Err(status) = csrf(&state, &headers, &body.csrf_token) {
        return deny(status);
    }
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    if let Err(error) = validate_control(&Control {
        action: body.action,
        generation: body.generation,
        expected_revision: body.expected_revision,
        percent: body.percent,
    }) {
        return api::error(error);
    }
    api::retired()
}

pub(super) async fn page(State(state): State<Runtime>, headers: HeaderMap) -> Response {
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    // Keep ordinary explicit book settings available under a fresh live role.
    let role = match state
        .accounts
        .session_actor(&session_token_digest(
            &state.cookies.session_token(&headers).unwrap_or_default(),
        ))
        .await
    {
        Ok(Some(role)) if role.administrator => role,
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let books = match state.publishing.manageable_books(role, 0).await {
        Ok(value) => value,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let mut body = String::from(
        "<section class=\"section\"><div class=\"container\"><h1>Настройки рекомендаций</h1>\
         <p data-testid=\"recommendation-model-retired\">Прежний подбор по истории чтения и сбор событий прекращены. \
         Сейчас показываются новые доступные книги с учётом ваших явных скрытий.</p>\
         <p><a href=\"/admin/audience\">Исторические отчёты аудитории</a></p>\
         <h2>Явные настройки книг</h2><p>Категории и ручные связи книг остаются обычными настройками. \
         Новый список книг проверяет текущие права доступа; ручные связи не определяют порядок этого списка.</p><ul>",
    );
    for (id, title) in books {
        let _ = write!(
            body,
            "<li><a href=\"/me/books/{id}/recommendations\">{}</a></li>",
            html::escape_html(&title),
        );
    }
    body.push_str("</ul></div></section>");
    pages::secure_page(
        html::layout("Настройки рекомендаций", "Явные настройки книг", &body),
        &[],
    )
}
