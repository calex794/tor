//! Contextual SSR cards and native preference forms. This adapter performs no
//! assignment, impression, click, book-open or individual audience write.
use super::{api, pages, ReaderContext, Runtime};
use axum::{
    body::{to_bytes, Body, HttpBody},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use tor_recommendations::domain::feedback::{book_id, Surface};

fn surface(path: &str, response: &Response) -> Option<(Surface, Option<i64>)> {
    if let Some(context) = response.extensions().get::<ReaderContext>() {
        return Some((Surface::ReaderEnd, Some(context.0)));
    }
    match path {
        "/" => Some((Surface::Home, None)),
        "/catalog" => Some((Surface::Catalog, None)),
        "/me/library" => Some((Surface::Library, None)),
        _ => path
            .strip_prefix("/books/")
            .and_then(|id| book_id(id).ok())
            .map(|id| (Surface::BookDetail, Some(id))),
    }
}

fn page_csrf(state: &Runtime, request: &HeaderMap, response: &HeaderMap) -> (String, Vec<String>) {
    // Reuse the normal page token so native preference forms remain valid.
    let name = if state.cookies.secure {
        "__Host-tor-csrf="
    } else {
        "tor-dev-csrf="
    };
    for cookie in response.get_all(header::SET_COOKIE) {
        if let Ok(cookie) = cookie.to_str() {
            if let Some(rest) = cookie.strip_prefix(name) {
                let token = rest.split(';').next().unwrap_or("");
                if tor_recommendations::domain::feedback::valid_token(token) {
                    return (token.to_owned(), Vec::new());
                }
            }
        }
    }
    state.cookies.csrf_for_page(request)
}

pub(super) async fn append(State(state): State<Runtime>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    let response = next.run(request).await;
    if !state.enabled
        || response.status() != StatusCode::OK
        || !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
    {
        return response;
    }
    let selected = surface(&path, &response);
    let selected = if selected.is_none() {
        let chapter = path
            .strip_prefix("/read/paid/")
            .or_else(|| path.strip_prefix("/read/"))
            .or_else(|| path.strip_prefix("/reader/"))
            .and_then(|id| book_id(id).ok());
        match chapter {
            Some(chapter) => match state.store.seed_for_chapter(chapter).await {
                Ok(Some(book)) => Some((Surface::ReaderEnd, Some(book))),
                _ => None,
            },
            None => None,
        }
    } else {
        selected
    };
    let Some((surface, seed)) = selected else {
        return response;
    };
    if response
        .body()
        .size_hint()
        .exact()
        .is_none_or(|size| size > 2 * 1024 * 1024)
    {
        return response;
    }
    let identity = match api::actor(&state, &headers).await {
        Ok(value) => Some(value),
        Err(StatusCode::UNAUTHORIZED) => None,
        Err(_) => return response,
    };
    // The new relation-weighted shelf is exclusive to the public free reader.
    // Paid readers and other REC surfaces retain their existing implementation.
    let free_chapter = path
        .strip_prefix("/reader/")
        .and_then(|id| book_id(id).ok());
    let is_shelf = surface == Surface::ReaderEnd && free_chapter.is_some() && seed.is_some();
    let feed_result = if is_shelf {
        state
            .store
            .reader_end_shelf(
                identity.as_ref().map(|(id, _)| *id),
                seed.expect("checked above"),
                free_chapter.expect("checked above"),
            )
            .await
    } else {
        api::load(&state, identity, surface, seed, 10).await
    };
    let feed = match feed_result {
        Ok(feed) if !feed.items.is_empty() => feed,
        _ => return response,
    };
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return crate::auth_cookies::deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let original = match std::str::from_utf8(&bytes) {
        Ok(original) => original,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };
    if original.contains("data-rec-panel") {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let at = if is_shelf {
        original.find("<!--READER_END_SHELF-->")
    } else {
        original.rfind("</main>")
    };
    let Some(at) = at else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    // An anonymous recommendation card has no account-setting form and does
    // not need a new CSRF cookie. Existing authentication tokens remain owned by A.
    let (csrf, cookies) = if identity.is_some() {
        page_csrf(&state, &headers, &parts.headers)
    } else {
        (String::new(), Vec::new())
    };
    let panel = if is_shelf {
        pages::shelf_panel(&feed)
    } else {
        pages::panel(&feed, &csrf)
    };
    let mut html = original.to_owned();
    if is_shelf {
        html = html.replace("<!--READER_END_SHELF-->", &panel);
    } else {
        html.insert_str(at, &panel);
    }
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            parts.headers.append(header::SET_COOKIE, value);
        }
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::ETAG);
    parts
        .headers
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Response::from_parts(parts, Body::from(html))
}
