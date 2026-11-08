//! NEwBOO book likes: session-only, origin+CSRF guarded, idempotent actions.
#![forbid(unsafe_code)]

use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::session_token_digest;
use tor_reading::repositories::{LikeMutation, PgBookLikes};

#[derive(Default, Deserialize)]
pub(super) struct LikeQuery {
    done: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct LikeForm {
    liked: String,
    csrf_token: Option<String>,
}

fn like_form(book_id: i64, liked: bool, csrf: &str) -> String {
    let (value, label) = if liked {
        ("0", "Убрать отметку «Нравится»")
    } else {
        ("1", "Нравится")
    };
    format!(
        "<form method=\"post\" action=\"/books/{book_id}/like\">\
         <input type=\"hidden\" name=\"liked\" value=\"{value}\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
         <button class=\"button\" type=\"submit\">{label}</button></form>",
        pages::escape_html(csrf)
    )
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    Query(query): Query<LikeQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let likes = PgBookLikes::new(state.accounts.pool().clone());
    let info = match likes.visible_summary(actor, id).await {
        Ok(Some(info)) => info,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let notice = match query.done.as_deref() {
        Some("added") => "<p role=\"status\">Отметка «Нравится» сохранена.</p>",
        Some("removed") => "<p role=\"status\">Отметка «Нравится» убрана.</p>",
        Some("repeat") => "<p role=\"status\">Количество отметок не изменилось.</p>",
        _ => "",
    };
    let body = format!(
        "<section class=\"section auth-section\"><div class=\"container\">\
         <div class=\"auth-card\"><h1>Нравится книга</h1>\
         <h2>{}</h2><p>Отметок «Нравится»: {}</p>\
         {notice}{}\
         <p><a href=\"/books/{id}\">Вернуться к книге</a></p></div>\
         </div></section>",
        pages::escape_html(&info.title),
        info.count,
        like_form(id, info.liked, &csrf),
    );
    private_response(
        pages::layout("Нравится книга", "Отметка читателя для книги.", &body),
        &cookies,
    )
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<LikeForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let liked = match form.liked.as_str() {
        "1" => true,
        "0" => false,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    // Shared PostgreSQL limit across web processes; retries count even when
    // a duplicate would not increase the public count. Separate from login.
    let key = session_token_digest(&format!("action:book_like:user:{actor}"));
    let allowed = match state.accounts.permit_auth_attempt(&key, 20, 60).await {
        Ok(allowed) => allowed,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !allowed {
        let mut response = deny(StatusCode::TOO_MANY_REQUESTS);
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            HeaderValue::from_static("60"),
        );
        return response;
    }
    let likes = PgBookLikes::new(state.accounts.pool().clone());
    let outcome = match likes.set_like(actor, id, liked).await {
        Ok(outcome) => outcome,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let done = match outcome {
        LikeMutation::NotVisible => return deny(StatusCode::NOT_FOUND),
        LikeMutation::Added => "added",
        LikeMutation::Removed => "removed",
        LikeMutation::AlreadyLiked | LikeMutation::AlreadyRemoved => "repeat",
    };
    private_response(Redirect::to(&format!("/books/{id}/like?done={done}")), &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn like_form_uses_server_identity_and_html_escapes_token() {
        let form = like_form(13, false, "\"<&");
        assert!(form.contains("action=\"/books/13/like\""));
        assert!(form.contains("name=\"liked\" value=\"1\""));
        assert!(form.contains("&quot;&lt;&amp;"));
        assert!(!form.contains("user_id"));
        assert!(like_form(13, true, "x").contains("value=\"0\""));
    }
}
