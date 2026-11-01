//! B ownership is checked in the repository; A role is resolved from the live session.
use super::{pages, Runtime};
use crate::{
    auth_cookies::{deny, private_response},
    pages as html,
};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::session_token_digest;
use tor_books::repositories::recommendation_policy::RecommendationPolicyError;
use tor_contracts::auth::SessionActor;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AudienceForm {
    audience: String,
    related: String,
    expected_revision: i64,
    csrf_token: String,
}
async fn actor(state: &Runtime, headers: &HeaderMap) -> Result<SessionActor, StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .accounts
        .session_actor(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)
}
fn csrf(state: &Runtime, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
fn denied(error: RecommendationPolicyError) -> Response {
    deny(match error {
        RecommendationPolicyError::Invalid => StatusCode::BAD_REQUEST,
        RecommendationPolicyError::NotFound => StatusCode::NOT_FOUND,
        RecommendationPolicyError::Conflict => StatusCode::CONFLICT,
        RecommendationPolicyError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    })
}
pub(super) async fn page(
    State(state): State<Runtime>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    let settings = match state.publishing.recommendation_settings(actor, id).await {
        Ok(value) => value,
        Err(error) => return denied(error),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let options = [
        ("unreviewed", "Не проверено — не рекомендовать"),
        ("general", "Для общей аудитории"),
        (
            "adult",
            "Только взрослым — не рекомендовать до проверки возрастных прав",
        ),
    ]
    .into_iter()
    .map(|(value, label)| {
        format!(
            "<option value=\"{value}\" {}>{label}</option>",
            if value == settings.audience {
                "selected"
            } else {
                ""
            }
        )
    })
    .collect::<String>();
    let related = settings
        .related
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let audience_note = match settings.audience.as_str() {
        "general" => "Базовая аудитория: рекомендации доступны только для опубликованной и незаблокированной книги с доступом basic.",
        "adult" => "Взрослая аудитория исключена до появления проверенной пользовательской политики доступа.",
        _ => "Книга исключена из рекомендаций по выбранной категории.",
    };
    let authorization_note = "Редактирование книги и глав не требует повторного допуска в рекомендации. Модерация, блокировки и права доступа действуют отдельно.";
    let body=format!("<section class=\"section\"><div class=\"container\"><h1>Рекомендации книги «{}»</h1><p>{audience_note}</p><p>{authorization_note}</p><p>Укажите действительную аудиторию. Видимость книги, блокировки и доступ к главам проверяются отдельно при каждой выдаче.</p><form method=\"post\" action=\"/me/books/{id}/recommendations\"><label>Аудитория <select name=\"audience\">{options}</select></label><label>Связанные публичные книги: не более 10 ID через запятую <input name=\"related\" value=\"{related}\" maxlength=\"220\"></label><input type=\"hidden\" name=\"expected_revision\" value=\"{}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><button type=\"submit\">Сохранить</button></form><p><a href=\"/me/books/{id}\">Назад к книге</a></p></div></section>",html::escape_html(&settings.title),settings.revision,html::escape_html(&csrf));
    pages::secure_page(
        html::layout("Рекомендации книги", "Аудитория и связанные книги", &body),
        &cookies,
    )
}
pub(super) async fn save(
    State(state): State<Runtime>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(body): Form<AudienceForm>,
) -> Response {
    if let Err(status) = csrf(&state, &headers, &body.csrf_token) {
        return deny(status);
    }
    let actor = match actor(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    let related = if body.related.trim().is_empty() {
        Vec::new()
    } else {
        let values = body
            .related
            .split(',')
            .map(|s| tor_recommendations::domain::feedback::book_id(s.trim()))
            .collect::<Result<Vec<_>, _>>();
        match values {
            Ok(values) => values,
            Err(_) => return deny(StatusCode::BAD_REQUEST),
        }
    };
    match state
        .publishing
        .update_recommendation_settings(actor, id, body.expected_revision, &body.audience, &related)
        .await
    {
        Ok(_) => {
            // Scheduling failure never rolls back an already committed B policy.
            let _ = state.store.request_refresh().await;
            private_response(
                Redirect::to(&format!("/me/books/{id}/recommendations")),
                &[],
            )
        }
        Err(error) => denied(error),
    }
}
