//! Shared public catalogue card projection, without HTTP route definitions.
use super::pages;
use axum::http::StatusCode;
use tor_books::services::{CatalogError, ListPublicBooks, PublicBookRepository};

pub(super) async fn public_cards(
    repo: &dyn PublicBookRepository,
    limit: usize,
) -> Result<Vec<pages::PublicBookCard>, StatusCode> {
    let result = ListPublicBooks::new(repo)
        .execute(limit)
        .await
        .map_err(|error| match error {
            CatalogError::InvalidLimit => StatusCode::BAD_REQUEST,
            CatalogError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })?;
    Ok(result
        .into_iter()
        .map(|book| pages::PublicBookCard {
            id: book.id(),
            title: book.title().to_owned(),
            language: book.key().target_language().to_owned(),
            author_name: None,
            genres: Vec::new(),
        })
        .collect())
}
