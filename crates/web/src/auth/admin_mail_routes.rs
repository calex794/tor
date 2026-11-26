//! Reviewed administrator mail routes, split from the 600-line auth module.
use super::{admin_mail, book_rooms, group_mail, mail, mail_api, AuthState};
use axum::{
    routing::{get, post},
    Router,
};

#[path = "admin_mail_cost/mod.rs"]
mod cost;

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .merge(cost::router())
        .route("/odmen/mail", get(admin_mail::index))
        .route("/odmen/mail/users/{user_id}", get(admin_mail::user))
        .route("/odmen/mail/{id}", get(admin_mail::conversation))
        .route("/odmen/mail/groups", get(admin_mail::groups))
        .route("/odmen/mail/groups/{id}", get(admin_mail::group))
        .route("/odmen/mail/book-rooms", get(admin_mail::book_rooms))
        .route("/odmen/mail/book-rooms/{id}", get(admin_mail::book_room))
}

pub(super) fn member_routes() -> Router<AuthState> {
    Router::new()
        .route("/mail", get(mail::index))
        .route("/mail/start/{recipient_user_id}", post(mail::start))
        .route("/mail/{id}", get(mail::conversation))
        .route("/mail/{id}/messages", get(mail::poll).post(mail::send))
        .route("/mail/{id}/block", post(mail::block))
        .route(
            "/mail/groups",
            get(group_mail::index).post(group_mail::create),
        )
        .route("/mail/groups/{id}", get(group_mail::detail))
        .route(
            "/mail/groups/{id}/messages",
            get(group_mail::poll).post(group_mail::send),
        )
        .route("/mail/groups/{id}/leave", post(group_mail::leave))
        .route("/mail/groups/{id}/transfer", post(group_mail::transfer))
        .route("/mail/groups/{id}/disband", post(group_mail::disband))
        .route("/books/{id}/room", get(book_rooms::page))
        .route("/books/{id}/room/join", post(book_rooms::join))
        .route("/books/{id}/room/messages", post(book_rooms::send))
        .route("/books/{id}/room/leave", post(book_rooms::leave))
        .route(
            "/api/v1/me/messages",
            get(mail_api::get).post(mail_api::post),
        )
        .route("/assets/mail.js", get(mail::script))
}
