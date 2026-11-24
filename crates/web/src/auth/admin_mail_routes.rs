//! Reviewed administrator mail routes, split from the 600-line auth module.
use super::{admin_mail, AuthState};
use axum::{routing::get, Router};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/odmen/mail", get(admin_mail::index))
        .route("/odmen/mail/users/{user_id}", get(admin_mail::user))
        .route("/odmen/mail/{id}", get(admin_mail::conversation))
        .route("/odmen/mail/groups", get(admin_mail::groups))
        .route("/odmen/mail/groups/{id}", get(admin_mail::group))
        .route("/odmen/mail/book-rooms", get(admin_mail::book_rooms))
        .route("/odmen/mail/book-rooms/{id}", get(admin_mail::book_room))
}
