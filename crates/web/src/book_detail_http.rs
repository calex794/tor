//! Public book detail renderer, separate from the routing shell.
//! Attaches the scoped Follow action only after verified session lookup.
use super::*;

pub(super) async fn page(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<TocQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    // Require the canonical numeric spelling before consulting public book data.
    if !tor_search::domain::seo_urls::BookUrl::from_id(id)
        .is_some_and(|book_url| book_url.matches_path(uri.path()))
    {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    }
    if let Some(reader) = state.reader.as_ref() {
        let limit = query.limit.unwrap_or(100);
        let book = match reader
            .public_book_page(id, limit, query.after.as_deref())
            .await
        {
            Ok(Some(book)) => book,
            Ok(None) => return book_error(&state, StatusCode::NOT_FOUND).await,
            Err(error) => return book_error(&state, reading_status(error)).await,
        };
        let has_cover = if let Some(delivery) = state.covers.as_ref() {
            match delivery.available_card_ids(&[book.id]).await {
                Ok(ids) => ids.contains(&book.id),
                Err(_) => return book_error(&state, StatusCode::SERVICE_UNAVAILABLE).await,
            }
        } else {
            false
        };
        if let Some(catalog) = state.postgres.as_ref() {
            let id = match i64::try_from(book.id) {
                Ok(id) => id,
                Err(_) => return book_error(&state, StatusCode::NOT_FOUND).await,
            };
            let store = tor_recommendations::repositories::PgBookStars::new(catalog.pool().clone());
            let stars = match store.visible_summary(id).await {
                Ok(Some(stars)) => stars,
                Ok(None) => return book_error(&state, StatusCode::NOT_FOUND).await,
                Err(_) => return book_error(&state, StatusCode::SERVICE_UNAVAILABLE).await,
            };
            let html = if state.paid_batch_browser_enabled {
                pages::book_detail_with_chapters_cover_stars_batch(
                    &book, limit, has_cover, &stars, true,
                )
            } else {
                pages::book_detail_with_chapters_cover_stars(&book, limit, has_cover, &stars)
            };
            return book_follow_page::attach(&state.follow_control, &headers, book.id, html).await;
        }
        let html = pages::book_detail_with_chapters_cover(&book, limit, has_cover);
        return book_follow_page::attach(&state.follow_control, &headers, book.id, html).await;
    }
    let repo = state.books.as_ref();
    let book = match GetPublicBook::new(repo).execute(id).await {
        Ok(Some(book)) => book,
        Ok(None) => return book_error(&state, StatusCode::NOT_FOUND).await,
        Err(_) => return book_error(&state, StatusCode::SERVICE_UNAVAILABLE).await,
    };
    let html = pages::book_detail(&pages::PublicBookCard {
        id: book.id(),
        title: book.title().into(),
        language: book.key().target_language().into(),
        author_name: None,
        genres: Vec::new(),
    });
    Html(html.0.replace("<!--BOOK_FOLLOW_ACTION-->", "")).into_response()
}
