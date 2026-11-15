//! Native advertiser/publisher console and separate daily/chapter APIs.
//! Composition only; authorization and money proofs remain in F/D ports.
mod admin;
mod api;
mod console;
pub(crate) mod display;
mod render;
mod wire;
use crate::auth_cookies::{private_response, CookiePolicy};
use axum::{
    extract::DefaultBodyLimit,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_ads::{
    domain::paid::AdFlags,
    repositories::paid::PgPaidAds,
    services::{console::AdConsoleStore, paid::PaidAds},
};
use tor_commerce::repositories::ad_posting::PgAdPosting;
use tor_contracts::ads::*;

#[derive(Clone)]
pub(super) struct Runtime {
    accounts: PgAccounts,
    cookies: CookiePolicy,
    catalog: Arc<dyn AdConsoleStore>,
    funding: Arc<dyn AdFundingPort>,
    flags: AdFlags,
}
impl Runtime {
    fn service(&self) -> PaidAds<'_> {
        PaidAds {
            funding: self.funding.as_ref(),
            flags: self.flags,
        }
    }
}
pub(crate) fn flags() -> AdFlags {
    let on = |name| std::env::var(name).as_deref() == Ok("1");
    AdFlags {
        daily_purchase: on("TOR_ADS_DAILY_PURCHASE_ENABLED"),
        daily_serving: on("TOR_ADS_DAILY_SERVING_ENABLED"),
        chapter_purchase: on("TOR_CHAPTER_ADS_PURCHASE_ENABLED"),
        planner: on("TOR_CHAPTER_ADS_PLANNER_ENABLED"),
        chapter_serving: on("TOR_CHAPTER_ADS_SERVING_ENABLED"),
        billable: on("TOR_CHAPTER_ADS_BILLABLE_ENABLED"),
        settlement: on("TOR_CHAPTER_ADS_SETTLEMENT_ENABLED"),
    }
}
pub(crate) fn routes(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let pool = accounts.pool().clone();
    let state = Runtime {
        accounts,
        cookies,
        catalog: Arc::new(PgPaidAds::new(pool.clone())),
        funding: Arc::new(PgAdPosting::new(pool)),
        flags: flags(),
    };
    Router::new()
        .route("/me/ads", get(console::page))
        .route("/me/ads/daily", post(console::daily))
        .route("/me/ads/chapter", post(console::chapter))
        .route("/me/ads/{product}/{id}/cancel", post(console::cancel))
        .route(
            "/me/ads/publisher/settings",
            post(console::publisher_settings),
        )
        .route("/me/ads/publisher/credit", post(console::publisher_credit))
        .route("/api/v1/ads/daily-offers", get(api::offers))
        .route("/api/v1/ads/daily-campaigns", post(api::daily))
        .route("/api/v1/me/ads/daily-campaigns", get(api::daily_campaigns))
        .route(
            "/api/v1/ads/daily-campaigns/{id}/cancel",
            post(api::cancel_daily),
        )
        .route("/api/v1/ads/chapter/packages", get(api::packages))
        .route("/api/v1/ads/chapter/campaigns", post(api::chapter))
        .route("/api/v1/ads/chapter/campaigns/{id}", get(api::campaign))
        .route(
            "/api/v1/me/ads/chapter-campaigns",
            get(api::chapter_campaigns),
        )
        .route("/api/v1/ads/chapter/impressions", post(api::impression))
        .route("/api/v1/ads/chapter/click", post(api::click))
        .route(
            "/api/v1/ads/chapter/campaigns/{id}/cancel",
            post(api::cancel_chapter),
        )
        .route(
            "/api/v1/me/chapter-ads/publisher-ledger",
            get(api::publisher),
        )
        .route(
            "/api/v1/me/chapter-ads/publisher-settings",
            post(api::publisher_settings),
        )
        .route(
            "/api/v1/me/chapter-ads/publisher-credit",
            post(api::publisher_credit),
        )
        .route("/admin/promotion/paid", get(admin::page))
        .route("/admin/promotion/paid/daily-offer", post(admin::daily))
        .route(
            "/admin/promotion/paid/chapter-package",
            post(admin::package),
        )
        .route(
            "/admin/promotion/paid/slot-policy",
            post(admin::slot_policy),
        )
        .route(
            "/admin/promotion/paid/{product}/{id}/cancel",
            post(admin::cancel),
        )
        .layer(DefaultBodyLimit::max(8192))
        .with_state(state)
}
pub(super) async fn actor(
    s: &Runtime,
    h: &HeaderMap,
    administrator: bool,
) -> Result<AdActor, StatusCode> {
    let token = s.cookies.session_token(h).ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let a = s
        .accounts
        .session_actor(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if administrator && !a.administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(AdActor {
        user_id: a.actor.user_id.get(),
        session_digest: digest,
    })
}
pub(super) fn csrf(s: &Runtime, h: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !s.cookies.same_origin(h) || !s.cookies.valid_csrf(h, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
pub(super) fn api_csrf(s: &Runtime, h: &HeaderMap) -> Result<(), StatusCode> {
    csrf(
        s,
        h,
        h.get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
    )
}
#[derive(Clone, Copy, Debug)]
pub(super) struct ApiError {
    pub status: StatusCode,
    pub code: Option<&'static str>,
}
impl From<StatusCode> for ApiError {
    fn from(status: StatusCode) -> Self {
        Self { status, code: None }
    }
}
pub(super) fn coded(status: StatusCode, code: &'static str) -> ApiError {
    ApiError {
        status,
        code: Some(code),
    }
}
pub(super) fn command_key(h: &HeaderMap) -> Result<String, ApiError> {
    use base64::Engine;
    let mut values = h.get_all("idempotency-key").iter();
    let value = values
        .next()
        .ok_or_else(|| coded(StatusCode::UNPROCESSABLE_ENTITY, "IDEMPOTENCY_KEY_REQUIRED"))?;
    let invalid = || coded(StatusCode::UNPROCESSABLE_ENTITY, "INVALID_IDEMPOTENCY_KEY");
    if values.next().is_some() {
        return Err(invalid());
    }
    let value = value.to_str().map_err(|_| invalid())?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| invalid())?;
    if value.len() != 43
        || bytes.len() != 32
        || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes) != value
    {
        return Err(invalid());
    }
    Ok(value.to_owned())
}
pub(super) fn ad_error(e: AdError) -> ApiError {
    let code = match e {
        AdError::IdempotencyConflict => Some("IDEMPOTENCY_CONFLICT"),
        AdError::Insufficient => Some("AD_INSUFFICIENT_FUNDS"),
        _ => None,
    };
    ApiError {
        status: status(e),
        code,
    }
}
pub(super) fn product(value: &str) -> Result<AdProduct, StatusCode> {
    match value {
        "daily" => Ok(AdProduct::Daily),
        "chapter" => Ok(AdProduct::Chapter),
        _ => Err(StatusCode::NOT_FOUND),
    }
}
pub(super) fn status(e: AdError) -> StatusCode {
    match e {
        AdError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        AdError::Denied => StatusCode::FORBIDDEN,
        AdError::NotFound => StatusCode::NOT_FOUND,
        AdError::Conflict | AdError::IdempotencyConflict | AdError::Insufficient => {
            StatusCode::CONFLICT
        }
        AdError::CapReached => StatusCode::TOO_MANY_REQUESTS,
        AdError::Expired => StatusCode::GONE,
        AdError::NotReady | AdError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}
pub(super) fn secure(body: impl IntoResponse, cookies: &[String]) -> Response {
    let mut r = private_response(body, cookies);
    r.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    r.headers_mut().insert("content-security-policy",HeaderValue::from_static("default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"));
    r
}
pub(crate) async fn script() -> Response {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../../assets/paid-ads.js"),
    )
        .into_response()
}
pub(crate) async fn stylesheet() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../../assets/paid-ads.css"),
    )
        .into_response()
}
pub(super) fn error(error: impl Into<ApiError>) -> Response {
    let failure = error.into();
    let code = failure.status;
    let (name, message) = match code {
        StatusCode::UNAUTHORIZED => ("AUTH_REQUIRED", "Войдите в аккаунт."),
        StatusCode::FORBIDDEN => (
            "AD_ACCESS_DENIED",
            "Недостаточно полномочий или проверка запроса не пройдена.",
        ),
        StatusCode::NOT_FOUND => ("AD_NOT_FOUND", "Кампания недоступна."),
        StatusCode::CONFLICT => (
            "AD_CONFLICT",
            "Кампания изменилась или средств недостаточно. Обновите кабинет.",
        ),
        StatusCode::TOO_MANY_REQUESTS => {
            ("AD_FREQUENCY_LIMIT", "Достигнут лимит показов или частоты.")
        }
        StatusCode::GONE => ("AD_OPPORTUNITY_EXPIRED", "Срок подтверждения показа истёк."),
        StatusCode::PAYLOAD_TOO_LARGE => ("REQUEST_TOO_LARGE", "Запрос слишком большой."),
        StatusCode::BAD_REQUEST => ("MALFORMED_REQUEST", "Неверный формат запроса."),
        StatusCode::UNPROCESSABLE_ENTITY => (
            "AD_INVALID_REQUEST",
            "Проверьте поля запроса и версию предложения.",
        ),
        _ => ("AD_UNAVAILABLE", "Операция временно недоступна."),
    };
    let name = failure.code.unwrap_or(name);
    let request_id = tor_accounts::services::security::generate_session_token();
    secure(
        (
            code,
            axum::Json(
                serde_json::json!({"error":{"code":name,"message":message,"request_id":request_id}}),
            ),
        ),
        &[],
    )
}

pub(super) fn native_error(code: StatusCode) -> Response {
    let message=match code{
  StatusCode::UNAUTHORIZED=>"Войдите в аккаунт, чтобы продолжить.",
  StatusCode::FORBIDDEN=>"Запрос отклонён. Обновите страницу и проверьте права доступа.",
  StatusCode::CONFLICT=>"Кампания изменилась или доступных средств недостаточно. Обновите кабинет перед повтором.",
  StatusCode::UNPROCESSABLE_ENTITY|StatusCode::BAD_REQUEST=>"Проверьте поля формы и действующую редакцию тарифа.",
  StatusCode::NOT_FOUND=>"Кампания недоступна.",
  _=>"Операция временно недоступна. Проверьте список кампаний перед повторной оплатой."};
    let body=format!("<div class=\"ads-console\"><h1>Операция не выполнена</h1><p role=\"alert\">{message}</p><a href=\"/me/ads\">Открыть кабинет рекламы</a></div>");
    secure(
        (
            code,
            crate::pages::layout("Реклама книг", "Состояние рекламной операции", &body),
        ),
        &[],
    )
}
