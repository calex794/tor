//! A-06: one validated profile service for HTML and JSON.
use super::{session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tor_accounts::{
    repositories::{
        profile::{Profile, ProfileError},
        PgAccounts,
    },
    services::profile::ProfileUpdate,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateForm {
    display_name: Option<String>,
    handle: Option<String>,
    adult_enabled: Option<bool>,
    expected_revision: i64,
    csrf_token: Option<String>,
}

#[derive(Serialize)]
struct ProfileJson {
    id: String,
    display_name: String,
    handle: Option<String>,
    adult_enabled: bool,
    revision: i64,
    delivery_settings: String,
    csrf_token: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorJson,
}
#[derive(Serialize)]
struct ErrorJson {
    code: &'static str,
    message: &'static str,
    request_id: String,
}
pub(super) fn api_error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    private_response(
        (
            status,
            Json(ErrorBody {
                error: ErrorJson {
                    code,
                    message,
                    request_id: tor_accounts::services::security::generate_session_token(),
                },
            }),
        ),
        &[],
    )
}

pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    let profile = match state.accounts.profile(actor).await {
        Ok(Some(profile)) => profile,
        _ => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::profile_settings(&profile, &csrf, None), &cookies)
}

pub(super) async fn api_get(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => {
            return api_error(
                StatusCode::UNAUTHORIZED,
                "AUTH_REQUIRED",
                "Войдите в учётную запись",
            )
        }
        Err(status) => return api_error(status, "UNAVAILABLE", "Повторите позже"),
    };
    let profile = match state.accounts.profile(actor).await {
        Ok(Some(profile)) => profile,
        _ => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "UNAVAILABLE",
                "Повторите позже",
            )
        }
    };
    let delivery = match state
        .notification_preferences
        .delivery_preference(actor)
        .await
    {
        Ok(mode) => mode.as_str().to_owned(),
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "UNAVAILABLE",
                "Повторите позже",
            )
        }
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(Json(profile_json(profile, delivery, csrf)), &cookies)
}

fn profile_json(p: Profile, delivery_settings: String, csrf_token: String) -> ProfileJson {
    ProfileJson {
        id: p.user_id.to_string(),
        display_name: p.display_name,
        handle: p.handle,
        adult_enabled: p.adult_enabled,
        revision: p.revision,
        delivery_settings,
        csrf_token,
    }
}

async fn save(
    accounts: &PgAccounts,
    actor: i64,
    form: UpdateForm,
) -> Result<i64, (StatusCode, &'static str, &'static str)> {
    let mut update = ProfileUpdate {
        display_name: form.display_name,
        handle: form.handle,
        adult_enabled: form.adult_enabled,
        expected_revision: form.expected_revision,
    };
    update
        .validate()
        .map_err(|message| (StatusCode::UNPROCESSABLE_ENTITY, "INVALID_PROFILE", message))?;
    accounts
        .update_profile(actor, &update)
        .await
        .map_err(|error| match error {
            ProfileError::Stale => (
                StatusCode::CONFLICT,
                "STALE_REVISION",
                "Настройки уже изменились. Обновите страницу",
            ),
            ProfileError::Duplicate => (
                StatusCode::CONFLICT,
                "HANDLE_UNAVAILABLE",
                "Этот адрес профиля недоступен",
            ),
            ProfileError::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "UNAVAILABLE",
                "Повторите позже",
            ),
        })
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<UpdateForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    let csrf = form.csrf_token.clone().unwrap_or_default();
    match save(&state.accounts, actor, form).await {
        Ok(_) => private_response(Redirect::to("/me/settings"), &[]),
        Err((status, _, message)) => match state.accounts.profile(actor).await {
            Ok(Some(profile)) => private_response(
                (
                    status,
                    pages::profile_settings(&profile, &csrf, Some(message)),
                ),
                &[],
            ),
            _ => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
        },
    }
}

#[derive(Serialize)]
struct RevisionJson {
    revision: i64,
}
pub(super) async fn api_patch(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(form): Json<UpdateForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return api_error(
            StatusCode::FORBIDDEN,
            "CSRF_REQUIRED",
            "Обновите страницу настроек",
        );
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => {
            return api_error(
                StatusCode::UNAUTHORIZED,
                "AUTH_REQUIRED",
                "Войдите в учётную запись",
            )
        }
        Err(status) => return api_error(status, "UNAVAILABLE", "Повторите позже"),
    };
    match save(&state.accounts, actor, form).await {
        Ok(revision) => private_response(Json(RevisionJson { revision }), &[]),
        Err((status, code, message)) => api_error(status, code, message),
    }
}
