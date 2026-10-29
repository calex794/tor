use crate::{
    schemas::BookView,
    services::{CatalogError, ListPublicBooks, PublicBookRepository, DEFAULT_PAGE_SIZE},
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
struct ApiState {
    repository: Arc<dyn PublicBookRepository>,
}
#[derive(Deserialize)]
struct CatalogQuery {
    limit: Option<usize>,
}

async fn list_books(
    State(state): State<ApiState>,
    Query(query): Query<CatalogQuery>,
) -> Result<Json<Vec<BookView>>, StatusCode> {
    let books = ListPublicBooks::new(state.repository.as_ref())
        .execute(query.limit.unwrap_or(DEFAULT_PAGE_SIZE))
        .await
        .map_err(|e| match e {
            CatalogError::InvalidLimit => StatusCode::BAD_REQUEST,
            CatalogError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })?;
    Ok(Json(
        books
            .into_iter()
            .map(|book| BookView {
                id: book.id(),
                title: book.title().to_owned(),
                target_language: book.key().target_language().to_owned(),
            })
            .collect(),
    ))
}
pub fn router(repository: Arc<dyn PublicBookRepository>) -> Router {
    Router::new()
        .route("/api/v1/books", get(list_books))
        .with_state(ApiState { repository })
}
