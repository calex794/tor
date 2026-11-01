//! Independently verified A administrator role; a normal reader session is insufficient.
use super::{api, pages, Runtime};
use crate::{
    auth_cookies::{deny, private_response},
    pages as html,
};
use axum::{
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    Json,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QualityQuery {
    days: Option<i32>,
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
    match state.store.quality(query.days.unwrap_or(7)).await {
        Ok(rows) => private_response(Json(rows), &[]),
        Err(error) => api::error(error),
    }
}
async fn apply(state: &Runtime, body: Control) -> Response {
    let result = match body.action {
        Action::Refresh => match state.store.request_refresh().await {
            Ok(id) => return private_response(Json(id), &[]),
            Err(error) => Err(error),
        },
        Action::Activate => {
            state
                .store
                .activate_generation(
                    body.generation.as_deref().unwrap_or(""),
                    body.expected_revision,
                    body.percent.unwrap_or(0),
                )
                .await
        }
        Action::Hold => state.store.hold(body.expected_revision).await,
        Action::Rollback => state.store.rollback(body.expected_revision).await,
    };
    match result {
        Ok(revision) => private_response(Json(revision), &[]),
        Err(error) => api::error(error),
    }
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
    apply(&state, body).await
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
    let response = apply(
        &state,
        Control {
            action: body.action,
            generation: body.generation,
            expected_revision: body.expected_revision,
            percent: body.percent,
        },
    )
    .await;
    if response.status().is_success() {
        private_response(Redirect::to("/me/recommendations/manage"), &[])
    } else {
        response
    }
}
pub(super) async fn page(State(state): State<Runtime>, headers: HeaderMap) -> Response {
    let user = match actor(&state, &headers).await {
        Ok(user) => user,
        Err(status) => return deny(status),
    };
    let status = match state.store.runtime_status().await {
        Ok(value) => value,
        Err(error) => return api::error(error),
    };
    let generations = match state.store.recent_generations().await {
        Ok(value) => value,
        Err(error) => return api::error(error),
    };
    let role = match state
        .accounts
        .session_actor(&session_token_digest(
            &state.cookies.session_token(&headers).unwrap_or_default(),
        ))
        .await
    {
        Ok(Some(role)) if role.administrator => role,
        _ => return deny(StatusCode::FORBIDDEN),
    };
    let books = match state.publishing.manageable_books(role, 0).await {
        Ok(value) => value,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut body=format!("<section class=\"section\"><div class=\"container\"><h1>Управление рекомендациями</h1><p>Активное поколение: {}. Доля: {}%. Пауза: {}. Сбор событий: {}.</p><p>Атрибуция — наблюдение, не доказательство причинного эффекта. Автоматические штрафы за отсутствие клика отключены.</p>",html::escape_html(status.0.as_deref().unwrap_or("нет")),status.3,status.4,state.collection);
    for (action, label) in [
        ("refresh", "Построить поколение"),
        ("hold", "Остановить выдачу поколения"),
        ("rollback", "Вернуть предыдущее"),
    ] {
        let _=write!(body,"<form method=\"post\" action=\"/me/recommendations/manage\"><input type=\"hidden\" name=\"action\" value=\"{action}\"><input type=\"hidden\" name=\"expected_revision\" value=\"{}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><button type=\"submit\">{label}</button></form>",status.2,html::escape_html(&csrf));
    }
    body.push_str("<h2>Последние поколения</h2>");
    for (id, build_status, count) in generations {
        let _ = write!(
            body,
            "<article><p>{}: {}; связей {count}</p>",
            html::escape_html(&id),
            html::escape_html(&build_status)
        );
        if build_status == "validated" {
            let _=write!(body,"<form method=\"post\" action=\"/me/recommendations/manage\"><input type=\"hidden\" name=\"action\" value=\"activate\"><input type=\"hidden\" name=\"generation\" value=\"{}\"><input type=\"hidden\" name=\"expected_revision\" value=\"{}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><label>Доля <select name=\"percent\"><option>0</option><option>1</option><option>5</option><option>10</option><option>25</option><option>50</option><option>100</option></select></label><button type=\"submit\">Активировать</button></form>",html::escape_html(&id),status.2,html::escape_html(&csrf));
        }
        body.push_str("</article>");
    }
    body.push_str("<h2>Категории книг</h2><p>Опубликованные книги базового уровня не требуют отдельного одобрения рекомендаций. Явные категории adult и unreviewed исключаются; блокировки и доступ проверяются в момент выдачи.</p><ul>");
    for (id, title) in books {
        let _ = write!(
            body,
            "<li><a href=\"/me/books/{id}/recommendations\">{}</a></li>",
            html::escape_html(&title)
        );
    }
    let _ = user; // Verified role above; no raw identity is rendered in analytics.
    body.push_str("</ul><p><a href=\"/api/v1/admin/recommendations/quality\">Ограниченная статистика JSON</a></p></div></section>");
    pages::secure_page(
        html::layout(
            "Управление рекомендациями",
            "Настройки и проверенные агрегаты",
            &body,
        ),
        &cookies,
    )
}
