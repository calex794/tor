//! The public JSON search route exposes only published book metadata.
use crate::{domain::SearchError, repositories::PgSearch};
use axum::{extract::{Query, State}, http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
struct SearchApiState {
    search: PgSearch,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery {
    q: Option<String>,
    genre: Option<String>,
    language: Option<String>,
    after: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct SearchItem {
    id: String,
    title: String,
    language: String,
}

#[derive(Serialize)]
struct SearchResponse {
    items: Vec<SearchItem>,
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    request_id: &'static str,
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

fn search_error(error: SearchError) -> (StatusCode, Json<ErrorEnvelope>) {
    let (status, code, message) = match error {
        SearchError::Invalid => (
            StatusCode::BAD_REQUEST,
            "INVALID_SEARCH_QUERY",
            "Invalid or unsupported search filter",
        ),
        SearchError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "SEARCH_UNAVAILABLE",
            "Search is temporarily unavailable",
        ),
    };
    (
        status,
        Json(ErrorEnvelope {
            error: ErrorBody {
                code,
                message,
                request_id: "",
            },
        }),
    )
}

/// F-03: the genre projection does not exist yet. Reject instead of
/// pretending to filter or returning unrelated books.
async fn search_books(
    State(state): State<SearchApiState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    if query.genre.as_ref().is_some_and(|genre| !genre.trim().is_empty()) {
        return Err(search_error(SearchError::Invalid));
    }
    let page = state
        .search
        .search_filtered(
            query.q.as_deref().unwrap_or(""),
            query.language.as_deref(),
            query.after.as_deref(),
            query.limit.unwrap_or(20),
        )
        .await
        .map_err(search_error)?;
    Ok(Json(SearchResponse {
        items: page.items.into_iter().map(|item| SearchItem {
            id: item.id.to_string(),
            title: item.title,
            language: item.language,
        }).collect(),
        next_cursor: page.next_cursor,
    }))
}

pub fn router(search: PgSearch) -> Router {
    Router::new()
        .route("/api/v1/search/books", get(search_books))
        .with_state(SearchApiState { search })
}
