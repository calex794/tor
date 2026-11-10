//! A distinct user choice for the broader, bounded FLOW visit scope.
use super::{api, Runtime};
use crate::{
    auth_cookies::{deny, private_response},
    pages as html,
};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_recommendations::domain::feedback::ConsentState;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FlowConsentForm {
    #[serde(default)]
    enabled: String,
    expected_revision: i64,
    command_key: String,
    csrf_token: String,
}

pub(super) fn form(state: &ConsentState, csrf: &str, recommendations_enabled: bool) -> String {
    let checked = if state.enabled { "checked" } else { "" };
    let prerequisite = if recommendations_enabled {
        ""
    } else {
        "<p>Сначала включите рекомендации в форме выше. Измерение визитов остаётся отдельным добровольным выбором.</p>"
    };
    format!(
        "<section aria-labelledby=\"flow-consent-title\"><h2 id=\"flow-consent-title\">Измерение визитов</h2>\
         <p>Отдельно разрешите учитывать входную и последнюю страницу по типу, первый наблюдаемый источник перехода, \
         посещения книг и читалки, подтверждённое чтение, сохранения, подписки на книги и возвращения. \
         Это помогает понять путь от входа на сайт до чтения. Учитываются только действия после входа в ваш аккаунт. \
         Полные адреса страниц, поисковые запросы, IP и текст книг не сохраняются.</p>\
         <p>Один наблюдаемый визит объединяет активность аккаунта, в том числе на разных устройствах, \
         пока между действиями нет перерыва в 30 минут. Это не измерение времени непрерывного чтения. \
         Подробные связи хранятся до 30 суток; обезличенные итоги — до 180 дней. \
         Отключение удаляет персональные связи визитов; старые обезличенные итоги могут сохраняться. \
         Отключение рекомендаций также отключает измерение визитов.</p>{prerequisite}\
         <form method=\"post\" action=\"/me/recommendations/flow-consent\" data-flow-consent>\
         <label><input type=\"checkbox\" name=\"enabled\" value=\"1\" {checked}> Разрешаю измерение моих визитов и связи с чтением</label>\
         <input type=\"hidden\" name=\"expected_revision\" value=\"{}\">\
         <input type=\"hidden\" name=\"command_key\" value=\"{}\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
         <button type=\"submit\">Сохранить настройку визитов</button></form></section>",
        state.revision, generate_session_token(), html::escape_html(csrf)
    )
}

pub(super) async fn submit(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(form): Form<FlowConsentForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let token = match state.cookies.session_token(&headers) {
        Some(token) => token,
        None => return deny(StatusCode::UNAUTHORIZED),
    };
    let actor = match state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
    {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let enabled = match form.enabled.as_str() {
        "1" => true,
        "" | "0" => false,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    match state
        .store
        .set_flow_consent(actor, enabled, form.expected_revision, &form.command_key)
        .await
    {
        Ok(_) => private_response(Redirect::to("/me/recommendations"), &[]),
        Err(error) => api::error(error),
    }
}
