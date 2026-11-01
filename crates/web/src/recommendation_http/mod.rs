//! H/I wiring for F recommendations. No SQL and no client-provided actor identity.
use crate::auth_cookies::CookiePolicy;
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
    Router,
};
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::recommendation_policy::PgRecommendationPolicy;
use tor_recommendations::repositories::PgRecommendations;

mod admin;
mod api;
mod author;
mod decorate;
mod organic;
mod pages;

#[derive(Clone)]
pub(crate) struct Runtime {
    pub(super) accounts: PgAccounts,
    pub(super) cookies: CookiePolicy,
    pub(super) store: PgRecommendations,
    pub(super) publishing: PgRecommendationPolicy,
    pub(super) enabled: bool,
    pub(super) collection: bool,
    pub(super) organic: bool,
}
impl Runtime {
    pub(crate) fn new(accounts: PgAccounts, cookies: CookiePolicy) -> Self {
        Self {
            store: PgRecommendations::new(accounts.pool().clone()),
            publishing: PgRecommendationPolicy::new(accounts.pool().clone()),
            accounts,
            cookies,
            enabled: std::env::var("TOR_RECOMMENDATIONS_ENABLED").as_deref() == Ok("1"),
            collection: std::env::var("TOR_REC_ATTRIBUTION_ENABLED").as_deref() == Ok("1"),
            organic: std::env::var("TOR_ORGANIC_ATTRIBUTION_ENABLED").as_deref() == Ok("1"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ReaderContext(pub i64);

pub(crate) fn routes(state: Runtime) -> Router {
    Router::new()
        .route("/api/v1/recommendations", get(api::feed))
        .route("/api/v1/books/{id}/recommendations", get(api::book_feed))
        .route(
            "/api/v1/recommendations/impressions",
            post(api::impressions),
        )
        .route("/api/v1/recommendations/activate", post(api::activate))
        .route("/api/v1/discovery/activate", post(organic::activate))
        .route(
            "/api/v1/me/recommendation-feedback",
            get(api::feedback_list).post(api::feedback),
        )
        .route("/me/recommendations", get(pages::page))
        .route("/me/recommendations/consent", post(pages::consent))
        .route("/me/recommendations/feedback", post(pages::feedback))
        .route("/api/v1/admin/recommendations/quality", get(admin::quality))
        .route(
            "/api/v1/admin/recommendations/control",
            post(admin::control),
        )
        .route(
            "/me/recommendations/manage",
            get(admin::page).post(admin::form_control),
        )
        .route(
            "/me/books/{id}/recommendations",
            get(author::page).post(author::save),
        )
        .route("/assets/recommendations.js", get(pages::script))
        .route("/assets/organic_discovery.js", get(organic::script))
        .layer(DefaultBodyLimit::max(32 * 1024))
        .with_state(state)
}

pub(crate) fn instrument(app: Router, state: Option<Runtime>) -> Router {
    match state {
        Some(state) if state.enabled => {
            app.layer(middleware::from_fn_with_state(state, decorate::append))
        }
        _ => app,
    }
}
