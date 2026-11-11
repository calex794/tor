//! F-SURPRISE: session-scoped discovery with a default-OFF HTTP gate.
use super::{session_user, AuthState};
use crate::auth_cookies::{deny, private_response};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tor_recommendations::repositories::surprises::{PgSurprises, PlanState, SurpriseError};

pub(crate) fn enabled() -> bool {
    configured_key().is_some()
}

fn configured_key() -> Option<[u8; 32]> {
    if std::env::var("TOR_SURPRISE_HTTP_ENABLED").as_deref() != Ok("1") {
        return None;
    }
    let encoded = std::env::var("TOR_SURPRISE_HMAC_KEY").ok()?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    decoded.try_into().ok()
}

fn problem(error: SurpriseError) -> Response {
    let status = match error {
        SurpriseError::Invalid => StatusCode::BAD_REQUEST,
        SurpriseError::Forbidden => StatusCode::FORBIDDEN,
        SurpriseError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    deny(status)
}

fn valid_write(state: &AuthState, headers: &HeaderMap) -> bool {
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    state.cookies.same_origin(headers) && state.cookies.valid_csrf(headers, csrf)
}

#[derive(Serialize)]
struct PlanResponse {
    state: &'static str,
    utc_day: Option<i64>,
    active_after_seconds: Option<i32>,
    csrf_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimRequest {
    utc_day: i64,
    active_seconds: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenRequest {
    utc_day: i64,
}

#[derive(Serialize)]
struct ClaimResponse {
    accepted: bool,
}

#[derive(Serialize)]
struct OpenResponse {
    id: i64,
    kind: String,
    title: String,
    body: String,
    book_id: Option<i64>,
    editorial: bool,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/api/v1/me/surprises/plan", get(plan))
        .route("/api/v1/me/surprises/claim", post(claim))
        .route("/api/v1/me/surprises/open", post(open))
        .route("/api/v1/me/surprises/disable", post(disable))
        .route("/assets/surprise.js", get(script))
        .route("/assets/surprise.css", get(styles))
}

async fn plan(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(key) = configured_key() else {
        return deny(StatusCode::NOT_FOUND);
    };
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let repository = PgSurprises::new(state.accounts.pool().clone());
    let (state_name, utc_day, delay) = match repository.plan(actor, &key).await {
        Ok(PlanState::Off) => ("off", None, None),
        Ok(PlanState::NotSelected) => ("none", None, None),
        Ok(PlanState::Planned {
            utc_day,
            active_after_seconds,
        }) => ("planned", Some(utc_day), Some(active_after_seconds)),
        Err(error) => return problem(error),
    };
    let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        Json(PlanResponse {
            state: state_name,
            utc_day,
            active_after_seconds: delay,
            csrf_token,
        }),
        &cookies,
    )
}

async fn claim(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(body): Json<ClaimRequest>,
) -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    if !state.cookies.same_origin(&headers) || !valid_write(&state, &headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let repository = PgSurprises::new(state.accounts.pool().clone());
    match repository
        .claim(actor, body.utc_day, body.active_seconds)
        .await
    {
        Ok(accepted) => private_response(Json(ClaimResponse { accepted }), &[]),
        Err(error) => problem(error),
    }
}

async fn open(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(body): Json<OpenRequest>,
) -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    if !state.cookies.same_origin(&headers) || !valid_write(&state, &headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let repository = PgSurprises::new(state.accounts.pool().clone());
    match repository.open(actor, body.utc_day).await {
        Ok(Some(item)) => private_response(
            Json(OpenResponse {
                id: item.id,
                kind: item.kind,
                title: item.title,
                body: item.body,
                book_id: item.book_id,
                editorial: item.editorial,
            }),
            &[],
        ),
        Ok(None) => deny(StatusCode::NOT_FOUND),
        Err(error) => problem(error),
    }
}

async fn disable(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    if !state.cookies.same_origin(&headers) || !valid_write(&state, &headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match PgSurprises::new(state.accounts.pool().clone())
        .disable(actor)
        .await
    {
        Ok(()) => private_response(StatusCode::NO_CONTENT, &[]),
        Err(error) => problem(error),
    }
}

async fn script() -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    private_response(
        (
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
            include_str!("../../assets/surprise.js"),
        ),
        &[],
    )
}

async fn styles() -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    private_response(
        (
            [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
            include_str!("../../assets/surprise.css"),
        ),
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_is_off_without_explicit_opt_in_and_strong_secret() {
        if std::env::var("TOR_SURPRISE_HTTP_ENABLED").as_deref() != Ok("1") {
            assert!(!enabled());
        }
    }
}
