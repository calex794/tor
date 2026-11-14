//! S032: owner-only Layer management, no payment endpoint.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, author_pages};
use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
    Router,
};
use tor_books::services::authoring::OwnedBook;
use tor_commerce::{repositories::layers::PgLayers, services::layers::LayerService};
use tor_contracts::paid_delivery::PositiveId;

mod layer_books;
mod layer_create;
mod layer_offer_manage;

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .merge(layer_books::routes())
        .merge(layer_create::routes())
        .merge(layer_offer_manage::routes())
}

pub(super) async fn author_index(
    state: &AuthState,
    actor: i64,
    books: &[OwnedBook],
    headers: &HeaderMap,
) -> Response {
    let (csrf, cookies) = state.cookies.csrf_for_page(headers);
    if !layer_books::enabled() {
        return private_response(author_pages::index(books, &csrf), &cookies);
    }
    let actor = match PositiveId::new(actor) {
        Ok(id) => id,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    match service.owned_layers(actor, 100).await {
        Ok(layers) => private_response(
            author_pages::index_with_layers(books, &layers, &csrf),
            &cookies,
        ),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
