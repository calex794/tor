//! Native editorial administration and optional public placements. No SQL here.
mod admin;
mod observation;
pub(crate) use observation::guard as observation_guard;
pub(crate) mod display;
mod render;
use crate::auth_cookies::{private_response, CookiePolicy};
use axum::{
    extract::DefaultBodyLimit,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_ads::{
    domain::editorial::{Administrator, EditorialError},
    repositories::editorial::PgEditorial,
};

pub(crate) const PATH: &str = "/admin/promotion/editorial";
#[derive(Clone)]
struct Runtime {
    accounts: PgAccounts,
    cookies: CookiePolicy,
    store: PgEditorial,
    enabled: bool,
}
pub(crate) fn routes(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let store = PgEditorial::new(accounts.pool().clone());
    Router::new()
        .route(
            "/admin/promotion/editorial",
            get(admin::page).post(admin::submit),
        )
        .layer(DefaultBodyLimit::max(4096))
        .with_state(Runtime {
            accounts,
            cookies,
            store,
            enabled: enabled(),
        })
}
pub(crate) fn enabled() -> bool {
    std::env::var("TOR_EDITORIAL_PROMOTION_ENABLED").as_deref() == Ok("1")
}
async fn administrator(state: &Runtime, headers: &HeaderMap) -> Result<Administrator, StatusCode> {
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
    Ok(Administrator {
        user_id: actor.actor.user_id.get(),
        session_digest: digest,
    })
}
fn secure(body: impl IntoResponse, cookies: &[String]) -> Response {
    let mut response = private_response(body, cookies);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}
fn status(error: EditorialError) -> StatusCode {
    match error {
        EditorialError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        EditorialError::Denied => StatusCode::FORBIDDEN,
        EditorialError::NotFound => StatusCode::NOT_FOUND,
        EditorialError::Conflict => StatusCode::CONFLICT,
        EditorialError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}
