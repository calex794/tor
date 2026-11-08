use super::{
    notification_settings, password_change, password_reset, profile_http, session_security,
    telegram_recovery, AuthState,
};
use axum::{
    routing::{get, post},
    Router,
};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/me/security/telegram", get(telegram_recovery::link_page))
        .route(
            "/me/security/telegram/unlink",
            post(telegram_recovery::unlink),
        )
        .route(
            "/me/security/telegram/begin",
            post(telegram_recovery::begin_link),
        )
        .route(
            "/me/security/telegram/confirm",
            post(telegram_recovery::confirm_link),
        )
        .route(
            "/recover/backup",
            get(telegram_recovery::backup_page).post(telegram_recovery::backup_submit),
        )
        .route("/assets/account-reset.js", get(password_reset::script))
        .route(
            "/me/security/sessions",
            get(session_security::page).post(session_security::revoke),
        )
        .route("/api/v1/me/password", post(password_change::api_submit))
        .route(
            "/forgot-password",
            get(password_reset::request_page).post(password_reset::request),
        )
        .route(
            "/reset-password",
            get(password_reset::confirm_page).post(password_reset::confirm),
        )
        .route(
            "/me/settings",
            get(profile_http::page).post(profile_http::submit),
        )
        .route(
            "/api/v1/me",
            get(profile_http::api_get).patch(profile_http::api_patch),
        )
        .route(
            "/me/notification-settings",
            get(notification_settings::page).post(notification_settings::save),
        )
}
