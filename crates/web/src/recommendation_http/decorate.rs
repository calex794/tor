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
use tor_recommendations::domain::feedback::{book_id, OrganicSource, Surface};

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
/// The generated catalogue uses this exact anchor shape. Escaped book titles
/// cannot forge it, and IDs are checked before issuing any server-side ticket.
fn organic_book_ids(html: &str) -> Vec<i64> {
    const PREFIX: &str = "class=\"book-mini-card__link\" href=\"/books/";
    let mut remaining = html;
    let mut ids = Vec::new();
    while let Some(start) = remaining.find(PREFIX) {
        remaining = &remaining[start + PREFIX.len()..];
        let Some((text, _)) = remaining.split_once('"') else {
            break;
        };
        if let Ok(id) = book_id(text) {
            if !ids.contains(&id) {
                ids.push(id);
                if ids.len() == 20 {
                    break;
                }
            }
        }
    }
    ids
}

fn organic_source(query: &str) -> OrganicSource {
    let filtered = query.split('&').any(|part| {
        let Some((key, value)) = part.split_once('=') else {
            return false;
        };
        !value.is_empty() && matches!(key, "q" | "language" | "genre" | "tag" | "fandom")
    });
    if filtered {
        OrganicSource::Search
    } else {
        OrganicSource::Catalog
    }
}

fn annotate_organic_links(mut html: String, tickets: &[(i64, String)]) -> String {
    for (id, ticket) in tickets {
        let original = format!("class=\"book-mini-card__link\" href=\"/books/{id}\"");
        let tagged = format!("{original} data-organic-ticket=\"{ticket}\"");
        html = html.replace(&original, &tagged);
    }
    html
}

pub(super) async fn append(State(state): State<Runtime>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or_default().to_owned();
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
    if state.collection && surface == Surface::BookDetail {
        if let (Some((actor, digest)), Some(book)) = (identity, seed) {
            let _ = state.store.record_book_open(actor, &digest, book).await;
            if state.organic {
                let _ = state
                    .store
                    .record_organic_book_open(actor, &digest, book)
                    .await;
            }
        }
    }
    let organic_eligible =
        state.collection && state.organic && surface == Surface::Catalog && identity.is_some();
    let feed = api::load(&state, identity, surface, seed, 10)
        .await
        .ok()
        .filter(|feed| !feed.items.is_empty());
    if feed.is_none() && !organic_eligible {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return crate::auth_cookies::deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let original = match std::str::from_utf8(&bytes) {
        Ok(original) => original,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };
    if !original.contains("</main>") {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let (csrf, cookies) = page_csrf(&state, &headers, &parts.headers);
    let mut html = original.to_owned();
    let mut organic_tagged = false;
    if organic_eligible {
        if let Some((actor, digest)) = identity {
            let ids = organic_book_ids(&html);
            if !ids.is_empty() {
                if let Ok(tickets) = state
                    .store
                    .issue_organic_tickets(actor, &digest, organic_source(&query), &ids)
                    .await
                {
                    if !tickets.is_empty() {
                        html = annotate_organic_links(html, &tickets);
                        organic_tagged = true;
                    }
                }
            }
        }
    }

    let mut panels = String::new();
    if let Some(feed) = feed {
        if !html.contains("data-rec-panel") {
            panels.push_str(&pages::panel(&feed, &csrf));
        }
    }
    if organic_tagged {
        panels.push_str(&format!(
            "<span hidden data-organic-csrf=\"{csrf}\"></span><script defer src=\"/assets/organic_discovery.js\"></script>"
        ));
    }
    if panels.is_empty() {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let Some(at) = html.rfind("</main>") else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    html.insert_str(at, &panels);
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
    Response::from_parts(parts, Body::from(html))
}

#[cfg(test)]
mod organic_catalog_tests {
    use super::{annotate_organic_links, organic_book_ids, organic_source};
    use tor_recommendations::domain::feedback::OrganicSource;

    #[test]
    fn only_actual_canonical_book_card_anchors_are_ticketed() {
        let html = concat!(
            "<a class=\"book-mini-card__link\" href=\"/books/42\">OK</a>",
            "<a class=\"book-mini-card__link\" href=\"/books/042\">Bad</a>",
            "<a class=\"book-mini-card__link\" href=\"//example.test\">Bad</a>",
            "<a class=\"book-mini-card__link\" href=\"/books/42\">Repeat</a>",
            "<a data-rec-link href=\"/books/7\">Not organic</a>"
        );
        assert_eq!(organic_book_ids(html), [42]);
        let tagged = annotate_organic_links(html.to_owned(), &[(42, "x".repeat(43))]);
        assert_eq!(tagged.matches("data-organic-ticket").count(), 2);
        assert!(!tagged.contains("href=\"/books/042\" data-organic-ticket"));
        assert!(!tagged.contains("data-rec-link data-organic-ticket"));
    }

    #[test]
    fn filters_are_distinct_from_catalog_sort_and_pagination() {
        assert_eq!(organic_source(""), OrganicSource::Catalog);
        assert_eq!(organic_source("sort=newest&page=2"), OrganicSource::Catalog);
        assert_eq!(organic_source("q="), OrganicSource::Catalog);
        assert_eq!(organic_source("q=magic"), OrganicSource::Search);
        assert_eq!(
            organic_source("genre=fantasy&sort=oldest"),
            OrganicSource::Search
        );
        assert_eq!(organic_source("tag=system"), OrganicSource::Search);
    }
}
