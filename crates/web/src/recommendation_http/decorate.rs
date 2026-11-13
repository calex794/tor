//! Contextual SSR cards and native preference forms. This adapter performs no
//! assignment, book-open or individual audience write. Anonymous observation
//! proofs are issued through a separate bounded queue when attached by main.
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

// The editorial chapter-end block is the same physical five-card shelf.
// Reject a separate/unrecognized editorial layout rather than adding a second panel.
fn reader_shelf_slot_available(html: &str) -> bool {
    html.contains("<!--READER_END_SHELF-->")
        && (!html.contains("data-editorial-campaign=")
            || html.contains("<!--READER_END_ORGANIC_CARDS-->"))
}

fn editorial_shelf_book_ids(html: &str) -> Vec<i64> {
    if !html.contains("<!--READER_END_ORGANIC_CARDS-->") {
        return Vec::new();
    }
    html.split("data-editorial-book=\"")
        .skip(1)
        .take(4)
        .filter_map(|part| part.split('"').next().and_then(|id| book_id(id).ok()))
        .collect()
}

// Both public-free reader spellings share the same single placement. Other
// /read/* paths include paid and private readers; they must never fall back
// to the generic recommendation panel at the end of <main>.
fn shelf_book_for_route(surface: Surface, path: &str, seed: Option<i64>) -> Option<(i64, i64)> {
    if surface != Surface::ReaderEnd {
        return None;
    }
    let chapter = path
        .strip_prefix("/reader/")
        .or_else(|| path.strip_prefix("/read/"))
        .and_then(|id| book_id(id).ok())?;
    seed.map(|book| (book, chapter))
}

pub(super) async fn append(State(state): State<Runtime>, request: Request, next: Next) -> Response {
    let issuer = request
        .extensions()
        .get::<std::sync::Arc<dyn tor_contracts::exposure::ObservationIssuer>>()
        .cloned();
    let searching = request.uri().query().is_some_and(|q| {
        q.split('&')
            .any(|pair| pair.starts_with("q=") && pair.len() > 2)
    });
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
    // On public free chapters the relation-weighted shelf replaces REC's
    // generic grid. Other surfaces keep the existing observed REC cards.
    let shelf_book = shelf_book_for_route(surface, &path, seed);
    // A paid/private reader has no free-chapter placement. Do not append the
    // generic grid through its <main> fallback, even with REC enabled.
    if surface == Surface::ReaderEnd && shelf_book.is_none() {
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
    let editorial_slot = shelf_book.is_some()
        && original.contains("<!--READER_END_ORGANIC_CARDS-->")
        && original.contains("data-editorial-campaign=");
    if (original.contains("data-rec-panel") && !editorial_slot)
        || (shelf_book.is_some() && !reader_shelf_slot_available(original))
    {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let at = if shelf_book.is_some() {
        original.find("<!--READER_END_SHELF-->")
    } else {
        original.rfind("</main>")
    };
    let Some(at) = at else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let editorial_ids = if editorial_slot {
        editorial_shelf_book_ids(original)
    } else {
        Vec::new()
    };
    // Never count editorial books twice, or exceed five visible cards.
    if editorial_slot && editorial_ids.is_empty() {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let feed_result = if let Some((book, chapter)) = shelf_book {
        state
            .store
            .reader_end_shelf_excluding(
                identity.as_ref().map(|(id, _)| *id),
                book,
                chapter,
                &editorial_ids,
                5 - editorial_ids.len(),
            )
            .await
    } else {
        api::load(&state, identity, surface, seed, 10).await
    };
    let feed = match feed_result {
        Ok(feed) if !feed.items.is_empty() => feed,
        _ => return Response::from_parts(parts, Body::from(bytes)),
    };
    // An anonymous recommendation card has no account-setting form and does
    // not need a new CSRF cookie. Existing authentication tokens remain owned by A.
    let (csrf, cookies) = if identity.is_some() {
        page_csrf(&state, &headers, &parts.headers)
    } else {
        (String::new(), Vec::new())
    };
    use tor_contracts::exposure::ExposureSurface as Observed;
    let observed = if path.starts_with("/read/paid/") {
        None
    } else {
        match surface {
            Surface::Home => Some(Observed::Home),
            Surface::Catalog => Some(if searching {
                Observed::Search
            } else {
                Observed::Catalog
            }),
            Surface::BookDetail => Some(Observed::BookDetail),
            Surface::ReaderEnd => Some(Observed::ReaderEnd),
            Surface::Library => None,
        }
    };
    let observer = issuer.as_deref().zip(observed);
    let panel = if editorial_slot {
        pages::shelf_cards_observed(&feed, issuer.as_deref())
    } else if shelf_book.is_some() {
        pages::shelf_panel_observed(&feed, issuer.as_deref())
    } else {
        pages::panel_observed(&feed, &csrf, observer)
    };
    let mut html = original.to_owned();
    if editorial_slot {
        html = html.replace("<!--READER_END_ORGANIC_CARDS-->", &panel);
        html = html.replace("<!--READER_END_SHELF-->", "");
    } else if shelf_book.is_some() {
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

#[cfg(test)]
mod shelf_precedence_tests {
    use super::{editorial_shelf_book_ids, reader_shelf_slot_available, shelf_book_for_route};
    use tor_recommendations::domain::feedback::Surface;

    #[test]
    fn only_free_chapter_route_shapes_can_own_the_shelf() {
        for path in ["/reader/41", "/read/41"] {
            assert_eq!(
                shelf_book_for_route(Surface::ReaderEnd, path, Some(7)),
                Some((7, 41)),
                "{path}"
            );
        }
        for path in [
            "/read/paid/41",
            "/read/admin/41",
            "/read/0",
            "/reader/0",
            "/reader/41/extra",
            "/books/41",
        ] {
            assert_eq!(
                shelf_book_for_route(Surface::ReaderEnd, path, Some(7)),
                None,
                "{path}"
            );
        }
        assert_eq!(
            shelf_book_for_route(Surface::ReaderEnd, "/reader/41", None),
            None
        );
        assert_eq!(
            shelf_book_for_route(Surface::BookDetail, "/reader/41", Some(7)),
            None
        );
    }

    #[test]
    fn editorial_and_organic_share_one_safe_five_card_placement() {
        assert!(reader_shelf_slot_available("<!--READER_END_SHELF-->"));
        let mixed = concat!(
            "<section data-rec-panel data-editorial-campaign=\"42\" ",
            "data-editorial-book=\"12\"></section>",
            "<!--READER_END_ORGANIC_CARDS--><!--READER_END_SHELF-->"
        );
        assert!(reader_shelf_slot_available(mixed));
        assert_eq!(editorial_shelf_book_ids(mixed), vec![12]);
        assert!(!reader_shelf_slot_available(
            "<section data-editorial-campaign=\"42\"></section><!--READER_END_SHELF-->"
        ));
        assert!(!reader_shelf_slot_available(
            "<section>paid reader</section>"
        ));
    }
}
