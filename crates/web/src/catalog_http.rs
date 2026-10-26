//! TOR F-02 catalogue HTTP projection, kept separate from routing shell.
use super::{cover_http, pages, public_cards, AppState};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Html,
};
use serde::Deserialize;
use tor_books::services::DEFAULT_PAGE_SIZE;

#[derive(Default, Deserialize)]
pub(super) struct CatalogQuery {
    q: Option<String>,
    language: Option<String>,
    genre: Option<String>,
    tag: Option<String>,
    fandom: Option<String>,
    after: Option<String>,
    sort: Option<String>,
}
pub(super) async fn catalog(
    State(state): State<AppState>,
    Query(query): Query<CatalogQuery>,
) -> Result<Html<String>, StatusCode> {
    if let Some(search) = state.search.as_ref() {
        // Always page the real PG catalog, even without a text filter.
        // Demo mode remains a separate legacy-compatible path.
        let sort = query.sort.as_deref().unwrap_or("oldest");
        let facets = tor_search::repositories::CatalogFacets {
            term: query.q.as_deref().unwrap_or(""),
            language: query.language.as_deref(),
            genre: query.genre.as_deref(),
            tag: query.tag.as_deref(),
            fandom: query.fandom.as_deref(),
        };
        let page = search
            .search_catalog(
                facets,
                query.after.as_deref(),
                DEFAULT_PAGE_SIZE,
                Some(sort),
            )
            .await
            .map_err(|error| match error {
                tor_search::SearchError::Invalid => StatusCode::BAD_REQUEST,
                tor_search::SearchError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            })?;
        // First page shows the exact count once. Cursor requests remain
        // cheap and do not perform a repeated full-filter count.
        let total = if query.after.is_none() {
            Some(
                search
                    .count_catalog(facets)
                    .await
                    .map_err(|error| match error {
                        tor_search::SearchError::Invalid => StatusCode::BAD_REQUEST,
                        tor_search::SearchError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                    })?,
            )
        } else {
            None
        };
        let options = if query.after.is_none() {
            Some(
                search
                    .taxonomy_options()
                    .await
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?,
            )
        } else {
            None
        };
        let cards: Vec<_> = page
            .items
            .into_iter()
            .map(|item| pages::PublicBookCard {
                id: item.id,
                title: item.title,
                language: item.language,
                author_name: if item.author_name.is_empty() {
                    None
                } else {
                    Some(item.author_name)
                },
                genres: item.genres,
            })
            .collect();
        let covers = cover_http::cover_card_ids(state.covers.as_ref(), &cards).await?;
        return Ok(pages::catalog_search_with_covers(
            &cards,
            pages::CatalogPageQuery {
                query: query.q.as_deref().unwrap_or(""),
                language: query.language.as_deref(),
                genre: query.genre.as_deref(),
                tag: query.tag.as_deref(),
                fandom: query.fandom.as_deref(),
                cursor: page.next_cursor.as_deref(),
                sort,
                total,
                options: options.as_ref(),
            },
            &covers,
        ));
    }
    if query.q.is_some()
        || query.after.is_some()
        || query.language.is_some()
        || query.genre.is_some()
        || query.tag.is_some()
        || query.fandom.is_some()
        || query.sort.is_some()
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(pages::catalog(
        &public_cards(state.books.as_ref(), DEFAULT_PAGE_SIZE).await?,
    ))
}
