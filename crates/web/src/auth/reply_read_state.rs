//! E-03: private reply read/unread state, no copied comment body.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_discussions::repositories::public_book_comments::ReadStateResult;

#[derive(Deserialize)]
struct ReplyReadStateForm {
    csrf_token: Option<String>,
    state: String,
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ReplyReadStateForm>,
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
    let read = match form.state.as_str() {
        "read" => true,
        "unread" => false,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let store = tor_discussions::repositories::public_book_comments::PgPublicBookComments::new(
        state.accounts.pool().clone(),
    );
    match store.set_reply_read_state(actor, id, read).await {
        ReadStateResult::Updated => private_response(Redirect::to("/me#me-replies-title"), &[]),
        ReadStateResult::NotFound => deny(StatusCode::NOT_FOUND),
        ReadStateResult::Unavailable => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
