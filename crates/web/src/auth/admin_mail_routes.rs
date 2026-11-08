//! Reviewed administrator mail routes, split from the 600-line auth module.
use super::{admin_mail, AuthState};
use axum::{routing::get, Router};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/admin/mail", get(admin_mail::index))
        .route("/admin/mail/users/{user_id}", get(admin_mail::user))
        .route("/admin/mail/{id}", get(admin_mail::conversation))
        .route("/admin/mail/groups", get(admin_mail::groups))
        .route("/admin/mail/groups/{id}", get(admin_mail::group))
        .route("/admin/mail/book-rooms", get(admin_mail::book_rooms))
        .route("/admin/mail/book-rooms/{id}", get(admin_mail::book_room))
}
