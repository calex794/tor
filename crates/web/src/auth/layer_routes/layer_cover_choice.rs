//! H/UI-18: choose a vetted existing Book cover without touching package purchases.
use super::{deny, layer_books, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::post,
    Router,
};
use serde::Deserialize;
use tor_commerce::{repositories::layers::PgLayers, services::layers::LayerService};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChoiceForm {
    csrf_token: String,
    expected_revision: i64,
    // Zero clears the Book choice, returning to the approved upload or default.
    cover_book_id: i64,
}

pub(super) fn routes() -> Router<AuthState> {
    Router::new().route(
        "/me/layers/{layer_id}/cover-choice",
        post(choose).layer(DefaultBodyLimit::max(4096)),
    )
}

async fn choose(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ChoiceForm>,
) -> Response {
    if !layer_books::enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let raw_actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    let (Ok(actor), Ok(layer)) = (PositiveId::new(raw_actor), PositiveId::new(raw_layer)) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let book = if form.cover_book_id == 0 {
        None
    } else {
        match PositiveId::new(form.cover_book_id) {
            Ok(book) => Some(book),
            Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        }
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    match service
        .choose_package_cover(actor, layer, book, form.expected_revision)
        .await
    {
        Ok(()) => private_response(
            Redirect::to(&format!("/me/layers/{raw_layer}/books?cover=selected")),
            &[],
        ),
        Err(error) => deny(layer_books::status(error)),
    }
}
