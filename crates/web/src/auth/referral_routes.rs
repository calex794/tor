//! F-09: referral links and dashboard routes, kept separate from account forms.
use super::{referral_alias, referral_capture, referral_dashboard, AuthState};
use axum::{
    routing::{get, post},
    Router,
};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/r/{code}", get(referral_capture::capture))
        .route("/go/{alias}", get(referral_capture::go))
        .route("/promo/{alias}", get(referral_capture::promo))
        .route(
            "/me/referrals",
            get(referral_dashboard::page).post(referral_dashboard::create),
        )
        .route(
            "/api/v1/me/referrals",
            get(referral_dashboard::api_get).post(referral_dashboard::api_create),
        )
        .route("/me/referrals/alias", post(referral_alias::browser_create))
        .route(
            "/api/v1/me/referrals/alias",
            post(referral_alias::api_create),
        )
}
