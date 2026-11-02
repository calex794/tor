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
    before: Option<String>,
    page: Option<u32>,
    sort: Option<String>,
}
/// Guard page-number presentation from impossible navigation states.
fn requested_page(query: &CatalogQuery) -> Result<u32, StatusCode> {
    // Compatibility: old /catalog?after=... URLs were always the next page.
    let page = query
        .page
        .unwrap_or(if query.after.is_some() { 2 } else { 1 });
    if (query.after.is_some() && query.before.is_some())
        || page == 0
        || page > 1_000_000
        || (query.after.is_some() && page < 2)
        || (query.before.is_some() && query.page.is_none())
        || (page > 1 && query.after.is_none() && query.before.is_none())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(page)
}

pub(super) async fn catalog(
    State(state): State<AppState>,
    Query(query): Query<CatalogQuery>,
) -> Result<Html<String>, StatusCode> {
    if let Some(search) = state.search.as_ref() {
        // Always page the real PG catalog, even without a text filter.
        // Demo mode remains a separate legacy-compatible path.
        let sort = query.sort.as_deref().unwrap_or("oldest");
        let page_number = requested_page(&query)?;
        let facets = tor_search::repositories::CatalogFacets {
            term: query.q.as_deref().unwrap_or(""),
            language: query.language.as_deref(),
            genre: query.genre.as_deref(),
            tag: query.tag.as_deref(),
            fandom: query.fandom.as_deref(),
        };
        let page = search
            .browse_catalog_page(
                facets,
                query.after.as_deref(),
                query.before.as_deref(),
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
        let total = if page_number == 1 {
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
        let options = if page_number == 1 {
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
                previous_cursor: page.previous_cursor.as_deref(),
                page_number,
                sort,
                total,
                options: options.as_ref(),
            },
            &covers,
        ));
    }
    if query.q.is_some()
        || query.after.is_some()
        || query.before.is_some()
        || query.page.is_some()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b_18_exactly_twenty_books_and_page_query_is_validated() {
        assert_eq!(DEFAULT_PAGE_SIZE, 20);
        let mut request = CatalogQuery::default();
        assert_eq!(requested_page(&request), Ok(1));
        request.page = Some(3);
        assert_eq!(requested_page(&request), Err(StatusCode::BAD_REQUEST));
        request.page = None;
        request.after = Some("20".to_owned());
        assert_eq!(requested_page(&request), Ok(2));
        request.page = Some(1);
        assert_eq!(requested_page(&request), Err(StatusCode::BAD_REQUEST));
        request.page = Some(2);
        assert_eq!(requested_page(&request), Ok(2));
        request.before = Some("21".to_owned());
        assert_eq!(requested_page(&request), Err(StatusCode::BAD_REQUEST));
        request.after = None;
        request.page = None;
        assert_eq!(requested_page(&request), Err(StatusCode::BAD_REQUEST));
        request.page = Some(1);
        assert_eq!(requested_page(&request), Ok(1));
        request.page = Some(1_000_001);
        assert_eq!(requested_page(&request), Err(StatusCode::BAD_REQUEST));
        request.before = None;
        request.page = Some(0);
        assert_eq!(requested_page(&request), Err(StatusCode::BAD_REQUEST));
    }
}
