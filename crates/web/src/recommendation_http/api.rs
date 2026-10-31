use super::Runtime;
use crate::auth_cookies::{deny, private_response};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_recommendations::domain::feedback::{
    book_id, valid_token, Choice, Feed, FeedbackError, ImpressionClaim, Surface,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FeedQuery {
    pub surface: Option<Surface>,
    pub seed: Option<i64>,
    pub limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImpressionBatch {
    pub items: Vec<ImpressionClaim>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Activation {
    pub ticket: String,
    pub csrf_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FeedbackCommand {
    pub book_id: String,
    pub choice: Choice,
    pub expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    pub after: Option<i64>,
}

pub(super) async fn actor(
    state: &Runtime,
    headers: &HeaderMap,
) -> Result<(i64, [u8; 32]), StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let id = state
        .accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok((id, digest))
}
pub(super) fn csrf(state: &Runtime, headers: &HeaderMap, posted: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, posted) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
fn csrf_header(headers: &HeaderMap) -> &str {
    headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorInfo,
}
#[derive(Serialize)]
struct ErrorInfo {
    code: &'static str,
    message: &'static str,
    request_id: String,
}
pub(super) fn error(value: FeedbackError) -> Response {
    let (status, code, message) = match value {
        FeedbackError::Invalid => (
            StatusCode::BAD_REQUEST,
            "INVALID_INPUT",
            "Некорректные данные",
        ),
        FeedbackError::Forbidden => (StatusCode::FORBIDDEN, "FORBIDDEN", "Действие недоступно"),
        FeedbackError::NotFound => (StatusCode::NOT_FOUND, "NOT_FOUND", "Не найдено"),
        FeedbackError::Conflict => (
            StatusCode::CONFLICT,
            "VERSION_CONFLICT",
            "Обновите страницу",
        ),
        FeedbackError::Limited => (
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
            "Слишком много запросов",
        ),
        FeedbackError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "UNAVAILABLE",
            "Рекомендации временно недоступны",
        ),
    };
    private_response(
        (
            status,
            Json(ErrorEnvelope {
                error: ErrorInfo {
                    code,
                    message,
                    request_id: generate_session_token(),
                },
            }),
        ),
        &[],
    )
}

fn idempotency_error(code: &'static str, message: &'static str) -> Response {
    private_response(
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorEnvelope {
                error: ErrorInfo {
                    code,
                    message,
                    request_id: generate_session_token(),
                },
            }),
        ),
        &[],
    )
}

/// 32 random bytes encoded as canonical 43-byte base64url without padding.
/// Last sextet must have its two unused bits set to zero.
fn idempotency_key(headers: &HeaderMap) -> Result<&str, Response> {
    let Some(raw) = headers.get("idempotency-key") else {
        return Err(idempotency_error(
            "IDEMPOTENCY_KEY_REQUIRED",
            "Не указан ключ повтора",
        ));
    };
    match raw.to_str() {
        Ok(token)
            if valid_token(token)
                && token
                    .as_bytes()
                    .last()
                    .is_some_and(|last| b"AEIMQUYcgkosw048".contains(last)) =>
        {
            Ok(token)
        }
        _ => Err(idempotency_error(
            "INVALID_IDEMPOTENCY_KEY",
            "Некорректный ключ повтора",
        )),
    }
}

pub(super) async fn load(
    state: &Runtime,
    identity: Option<(i64, [u8; 32])>,
    surface: Surface,
    seed: Option<i64>,
    limit: usize,
) -> Result<Feed, FeedbackError> {
    let actor = identity.as_ref().map(|(id, _)| *id);
    let feed = state.store.feed(actor, seed, limit).await?;
    if let Some((actor, digest)) = identity {
        if state.collection && state.store.consent(actor).await?.enabled {
            return match state
                .store
                .issue_tickets(actor, &digest, surface, feed)
                .await
            {
                Ok(feed) => Ok(feed),
                Err(FeedbackError::Forbidden) => state.store.feed(None, seed, limit).await,
                Err(FeedbackError::Limited) => state.store.feed(Some(actor), seed, limit).await,
                Err(error) => Err(error),
            };
        }
    }
    Ok(feed)
}

pub(super) async fn feed(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Response {
    if !state.enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let identity = match actor(&state, &headers).await {
        Ok(value) => Some(value),
        Err(StatusCode::UNAUTHORIZED) => None,
        Err(status) => return deny(status),
    };
    match load(
        &state,
        identity,
        query.surface.unwrap_or(Surface::Home),
        query.seed,
        query.limit.unwrap_or(20),
    )
    .await
    {
        Ok(feed) => private_response(Json(feed), &[]),
        Err(value) => error(value),
    }
}
pub(super) async fn book_feed(
    State(state): State<Runtime>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Response {
    if !state.enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let book = match book_id(&id) {
        Ok(id) => id,
        Err(value) => return error(value),
    };
    let identity = match actor(&state, &headers).await {
        Ok(value) => Some(value),
        Err(StatusCode::UNAUTHORIZED) => None,
        Err(status) => return deny(status),
    };
    match load(
        &state,
        identity,
        Surface::BookDetail,
        Some(book),
        query.limit.unwrap_or(20),
    )
    .await
    {
        Ok(feed) => private_response(Json(feed), &[]),
        Err(value) => error(value),
    }
}
pub(super) async fn impressions(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Json(body): Json<ImpressionBatch>,
) -> Response {
    if !state.enabled || !state.collection {
        return deny(StatusCode::FORBIDDEN);
    }
    if let Err(status) = csrf(&state, &headers, csrf_header(&headers)) {
        return deny(status);
    }
    let (actor, digest) = match actor(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    match state.store.impressions(actor, &digest, &body.items).await {
        Ok(()) => private_response(StatusCode::ACCEPTED, &[]),
        Err(value) => error(value),
    }
}
pub(super) async fn activate(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(body): Form<Activation>,
) -> Response {
    if !state.enabled || !state.collection {
        return deny(StatusCode::FORBIDDEN);
    }
    if let Err(status) = csrf(&state, &headers, &body.csrf_token) {
        return deny(status);
    }
    let (actor, digest) = match actor(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    match state.store.activate(actor, &digest, &body.ticket).await {
        Ok(id) => private_response(Redirect::to(&format!("/books/{id}")), &[]),
        Err(value) => error(value),
    }
}
pub(super) async fn feedback(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Json(body): Json<FeedbackCommand>,
) -> Response {
    if let Err(status) = csrf(&state, &headers, csrf_header(&headers)) {
        return deny(status);
    }
    let key = match idempotency_key(&headers) {
        Ok(key) => key,
        Err(response) => return response,
    };
    let (actor, _) = match actor(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    let book = match book_id(&body.book_id) {
        Ok(value) => value,
        Err(value) => return error(value),
    };
    match state
        .store
        .set_feedback(
            actor,
            book,
            body.choice,
            body.expected_revision,
            key,
        )
        .await
    {
        Ok(receipt) => private_response(Json(receipt), &[]),
        Err(value) => error(value),
    }
}
pub(super) async fn feedback_list(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Response {
    let (actor, _) = match actor(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    match state
        .store
        .hidden_books(actor, query.after.unwrap_or(0))
        .await
    {
        Ok(items) => private_response(Json(items), &[]),
        Err(value) => error(value),
    }
}
