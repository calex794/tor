//! Account-only one-tap demand; a browser POST must carry Origin, CSRF, session.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_reading::{repositories::demand::DemandError, services::PublicReadingRepository};

#[derive(Deserialize)]
pub(super) struct DemandForm {
    csrf_token: Option<String>,
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<DemandForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|v| state.cookies.valid_csrf(&headers, v))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let chapter = match state.reader.public_chapter(id).await {
        Ok(Some(chapter)) => chapter,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (Ok(book), Ok(chapter_id)) = (i64::try_from(chapter.book_id), i64::try_from(id)) else {
        return deny(StatusCode::NOT_FOUND);
    };
    match state
        .demand
        .record_verified_continuation(actor, book, chapter_id)
        .await
    {
        Ok(()) => private_response(Redirect::to(&format!("/read/{id}#continuation")), &[]),
        Err(DemandError::NotEligible) => deny(StatusCode::CONFLICT),
        Err(DemandError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(DemandError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
