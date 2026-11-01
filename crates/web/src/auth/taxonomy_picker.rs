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
    let items = match state
        .authoring
        .suggest_taxonomies(&query.kind, &query.q, query.limit.unwrap_or(15))
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|term| Suggestion {
                id: term.id,
                label: term.label,
                usage: term.usage,
            })
            .collect(),
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

/// Resolve author POST exclusively from canonical IDs. Keep the generated
/// compatibility TEXT[] projection until F/REC migrate their read contracts.
pub(super) async fn validated_author_book(
    state: &AuthState,
    actor: i64,
    form: &NewBookForm,
) -> Result<tor_books::services::authoring::NewBook, StatusCode> {
    use tor_books::services::authoring::{
        new_book_with_taxonomy, with_book_description, TaxonomyFields, TaxonomySelection,
    };
    // Raw labels are intentionally not a fallback: that would bypass the list.
    if [
        form.genres.as_deref(),
        form.tags.as_deref(),
        form.fandoms.as_deref(),
    ]
    .iter()
    .any(|value| !value.unwrap_or("").trim().is_empty())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let selected = TaxonomySelection::parse(
        form.genre_ids.as_deref(),
        form.tag_ids.as_deref(),
        form.fandom_ids.as_deref(),
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    let names = state
        .authoring
        .resolve_taxonomy_selection(&selected)
        .await
        .map_err(|err| match err {
            AuthorError::Invalid => StatusCode::BAD_REQUEST,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        })?;
    let identity = format!("manual:{actor}:{}", form.command_key);
    let mut request = new_book_with_taxonomy(
        &identity,
        &form.target_language,
        &form.title,
        &form.command_key,
        form.author_name.as_deref().unwrap_or(""),
        "",
        TaxonomyFields {
            tags: "",
            fandoms: "",
        },
    )
    .and_then(|book| with_book_description(book, form.description.as_deref().unwrap_or("")))
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    request.genres = names.genres;
    request.tags = names.tags;
    request.fandoms = names.fandoms;
    request.taxonomy_ids = selected;
    Ok(request)
}
