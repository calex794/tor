//! E-11 / S050: HTML and legacy widget transport share one support service.
use super::{deny, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tor_support::{
    domain::{Message, MessagePage, SupportError},
    repositories::postgres::PgSupport,
    services::SupportService,
};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/support", get(page))
        .route("/assets/newboo-support-widget.js", get(widget_script))
        .route("/assets/newboo-admin-support.js", get(admin_script))
        .route("/support/widget", get(widget))
        .route("/support/messages", get(messages).post(submit))
        .route("/api/v1/support/tickets", get(widget).post(json_submit))
        .merge(super::support_admin::router())
}
pub(super) fn service(state: &AuthState) -> SupportService<PgSupport> {
    SupportService::new(PgSupport::new(state.accounts.pool().clone()))
}
pub(super) fn status(error: SupportError) -> StatusCode {
    match error {
        SupportError::Invalid => StatusCode::BAD_REQUEST,
        SupportError::NotFound => StatusCode::NOT_FOUND,
        SupportError::Denied => StatusCode::FORBIDDEN,
        SupportError::Conflict => StatusCode::CONFLICT,
        SupportError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    pub(super) after_id: Option<i64>,
    pub(super) before_id: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SendForm {
    pub(super) csrf_token: String,
    pub(super) body: String,
    pub(super) client_message_id: Option<String>,
    #[serde(default)]
    pub(super) website: String,
}

#[derive(Serialize)]
pub(super) struct MessageView {
    message_id: String,
    sender_kind: String,
    body: String,
    created_at: String,
    is_own: bool,
}
impl MessageView {
    pub(super) fn new(message: Message, admin: bool) -> Self {
        let is_own = if admin {
            message.sender_kind == "admin"
        } else {
            message.sender_kind != "admin"
        };
        Self {
            message_id: message.id.to_string(),
            sender_kind: message.sender_kind,
            body: message.body,
            created_at: message.created_at,
            is_own,
        }
    }
}
#[derive(Serialize)]
struct Widget {
    ok: bool,
    conversation_id: String,
    identity_notice: String,
    csrf_token: String,
    messages: Vec<MessageView>,
    has_more: bool,
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let (who, mut cookies) = match super::support_identity::resolve(&state, &headers).await {
        Ok(c) => c,
        Err(s) => return deny(s),
    };
    let data = match service(&state)
        .messages(&who, query.after_id, query.before_id, 100)
        .await
    {
        Ok(d) => d,
        Err(e) => return deny(status(e)),
    };
    let (csrf, issued) = state.cookies.csrf_for_page(&headers);
    cookies.extend(issued);
    html_response(super::support_pages::chat(&data, &csrf, false), &cookies)
}

fn widget_view(data: MessagePage, csrf: String) -> Widget {
    let identity_notice = if data.chat.requester_id.is_some() {
        "Вы пишете из своей учётной записи.".into()
    } else {
        format!("Вы пишете как {}. Если войдёте, переписка сохранится. Без входа она удалится через 30 дней.",data.chat.guest_alias)
    };
    Widget {
        ok: true,
        conversation_id: data.chat.id.to_string(),
        identity_notice,
        csrf_token: csrf,
        messages: data
            .messages
            .into_iter()
            .map(|m| MessageView::new(m, false))
            .collect(),
        has_more: data.has_more,
    }
}

pub(super) async fn widget(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (who, mut cookies) = match super::support_identity::resolve(&state, &headers).await {
        Ok(c) => c,
        Err(s) => return deny(s),
    };
    let data = match service(&state).messages(&who, None, None, 100).await {
        Ok(d) => d,
        Err(e) => return deny(status(e)),
    };
    let (csrf, issued) = state.cookies.csrf_for_page(&headers);
    cookies.extend(issued);
    private_response(Json(widget_view(data, csrf)).into_response(), &cookies)
}

pub(super) async fn messages(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let (who, mut cookies) = match super::support_identity::resolve(&state, &headers).await {
        Ok(c) => c,
        Err(s) => return deny(s),
    };
    let data = match service(&state)
        .messages(&who, query.after_id, query.before_id, 100)
        .await
    {
        Ok(d) => d,
        Err(e) => return deny(status(e)),
    };
    let (csrf, issued) = state.cookies.csrf_for_page(&headers);
    cookies.extend(issued);
    private_response(Json(widget_view(data, csrf)).into_response(), &cookies)
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    Form(form): Form<SendForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    if !form.website.trim().is_empty() {
        return private_response(StatusCode::ACCEPTED, &[]);
    }
    let (who, cookies) = match super::support_identity::resolve(&state, &headers).await {
        Ok(c) => c,
        Err(s) => return deny(s),
    };
    if let Err(status) =
        super::support_identity::limit(&state, &who, &headers, peer.map(|p| p.0)).await
    {
        return deny(status);
    }
    let key = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .or(form.client_message_id.as_deref());
    match service(&state).send(&who, &form.body, key).await {
        Ok(message)
            if headers
                .get("accept")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.contains("application/json")) =>
        {
            #[derive(Serialize)]
            struct Receipt {
                ok: bool,
                message: MessageView,
            }
            private_response(
                Json(Receipt {
                    ok: true,
                    message: MessageView::new(message, false),
                })
                .into_response(),
                &cookies,
            )
        }
        Ok(_) => private_response(Redirect::to("/support"), &cookies),
        Err(SupportError::Invalid)
            if !headers
                .get("accept")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.contains("application/json")) =>
        {
            private_response(
                (
                    StatusCode::BAD_REQUEST,
                    super::support_pages::invalid_message(&form.body, &form.csrf_token, "/support"),
                ),
                &cookies,
            )
        }
        Err(e) => json_error(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JsonMessage {
    message: String,
}

pub(super) async fn json_submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    Json(input): Json<JsonMessage>,
) -> Response {
    if headers
        .get(axum::http::header::ORIGIN)
        .and_then(|h| h.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|t| state.cookies.valid_csrf(&headers, t))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let mut values = headers.get_all("idempotency-key").iter();
    let key = match (values.next().and_then(|h| h.to_str().ok()), values.next()) {
        (Some(key), None) if crate::auth_cookies::valid_token(key) => key,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let (who, cookies) = match super::support_identity::resolve(&state, &headers).await {
        Ok(c) => c,
        Err(s) => return deny(s),
    };
    if let Err(s) = super::support_identity::limit(&state, &who, &headers, peer.map(|p| p.0)).await
    {
        return deny(s);
    }
    match service(&state).json_send(&who, &input.message, key).await {
        Ok(message) => {
            #[derive(Serialize)]
            struct Receipt {
                ok: bool,
                message: MessageView,
            }
            private_response(
                Json(Receipt {
                    ok: true,
                    message: MessageView::new(message, false),
                })
                .into_response(),
                &cookies,
            )
        }
        Err(e) => json_error(e),
    }
}

pub(super) fn json_error(error: SupportError) -> Response {
    let (code, message) = match error {
        SupportError::Invalid => ("invalid", "Введите от 1 до 4000 символов."),
        SupportError::NotFound => ("not_found", "Переписка не найдена."),
        SupportError::Denied => ("forbidden", "Нет доступа к переписке."),
        SupportError::Conflict => (
            "conflict",
            "Этот ключ уже использован для другого сообщения.",
        ),
        SupportError::Unavailable => (
            "unavailable",
            "Поддержка временно недоступна. Повторите позже.",
        ),
    };
    #[derive(Serialize)]
    struct Error {
        ok: bool,
        error: &'static str,
        message: &'static str,
    }
    private_response(
        (
            status(error),
            Json(Error {
                ok: false,
                error: code,
                message,
            }),
        ),
        &[],
    )
}

pub(super) fn html_response(result: impl IntoResponse, cookies: &[String]) -> Response {
    let mut response = private_response(result, cookies);
    response.headers_mut().insert("content-security-policy",axum::http::HeaderValue::from_static("default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"));
    response
}
pub(super) async fn widget_script() -> impl IntoResponse {
    (
        [("content-type", "application/javascript; charset=utf-8")],
        include_str!("../../assets/newboo-support-widget.js"),
    )
}
pub(super) async fn admin_script() -> impl IntoResponse {
    (
        [("content-type", "application/javascript; charset=utf-8")],
        include_str!("../../assets/newboo-admin-support.js"),
    )
}
