//! Append one bounded first-party panel to successful SSR pages; never consume
//! unknown/streamed bodies or turn an analytics outage into a reading failure.
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
    // Reuse any token already issued by the real page so its forms keep working.
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
fn allow_first_party_scripts(headers: &mut HeaderMap) {
    let policy=headers.get("content-security-policy").and_then(|v|v.to_str().ok()).unwrap_or("default-src 'none'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'");
    let mut scripts = false;
    let mut connect = false;
    let mut directives = Vec::new();
    for directive in policy.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        if directive.starts_with("script-src ") {
            scripts = true;
            directives.push(if directive.contains("'none'") {
                "script-src 'self'".to_owned()
            } else if directive.contains("'self'") {
                directive.to_owned()
            } else {
                format!("{directive} 'self'")
            });
        } else if directive.starts_with("connect-src ") {
            connect = true;
            directives.push(if directive.contains("'none'") {
                "connect-src 'self'".to_owned()
            } else if directive.contains("'self'") {
                directive.to_owned()
            } else {
                format!("{directive} 'self'")
            });
        } else {
            directives.push(directive.to_owned());
        }
    }
    if !scripts {
        directives.push("script-src 'self'".to_owned());
    }
    if !connect {
        directives.push("connect-src 'self'".to_owned());
    }
    if let Ok(value) = HeaderValue::from_str(&directives.join("; ")) {
        headers.insert("content-security-policy", value);
    }
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
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/html"))
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
    if state.collection && surface == Surface::BookDetail {
        if let (Some((actor, digest)), Some(book)) = (identity, seed) {
            let _ = state.store.record_book_open(actor, &digest, book).await;
        }
    }
    let feed = match api::load(&state, identity, surface, seed, 10).await {
        Ok(feed) if !feed.items.is_empty() => feed,
        _ => return response,
    };
    let (csrf, cookies) = page_csrf(&state, &headers, response.headers());
    let panel = pages::panel(&feed, &csrf);
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return crate::auth_cookies::deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };
    if text.contains("data-rec-panel") {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let Some(at) = text.rfind("</main>") else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let mut result = String::with_capacity(text.len() + panel.len());
    result.push_str(&text[..at]);
    result.push_str(&panel);
    result.push_str(&text[at..]);
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
    allow_first_party_scripts(&mut parts.headers);
    Response::from_parts(parts, Body::from(result))
}
