//! Authenticated, indexed taxonomy suggestion endpoint and local picker asset.
use super::*;
use axum::{extract::Query, http::header, response::IntoResponse, Json};
use serde::Serialize;

#[derive(Deserialize)]
pub(super) struct SuggestQuery {
    kind: String,
    q: String,
    limit: Option<u8>,
}
#[derive(Serialize)]
struct Suggestion {
    id: i64,
    label: String,
    usage: i64,
}
#[derive(Serialize)]
struct SuggestionResult {
    items: Vec<Suggestion>,
}
pub(super) async fn suggest(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<SuggestQuery>,
) -> Response {
    match session_user(&state, &headers).await {
        Ok(Some(_)) => (),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let items = match state.authoring.suggest_taxonomies(
        &query.kind,
        &query.q,
        query.limit.unwrap_or(15),
    ).await {
        Ok(rows) => rows.into_iter().map(|term| Suggestion {
            id: term.id,
            label: term.label,
            usage: term.usage,
        }).collect(),
        Err(AuthorError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    private_response(Json(SuggestionResult { items }), &[])
}

pub(super) async fn script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../../assets/taxonomy-picker.js"),
    )
}
