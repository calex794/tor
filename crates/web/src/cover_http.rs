//! Thin HTTP adapter and optional router constructors for private book covers.
use super::{
    auth_cookies, book_error, book_follow_page, cover_delivery, make_router, pages, ril, AppState,
};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::Response,
    Json, Router,
};
use std::sync::Arc;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;

/// Only render a cover when B reports an attached visible variant.
pub(super) async fn cover_card_ids(
    delivery: Option<&cover_delivery::CoverDelivery>,
    cards: &[pages::PublicBookCard],
) -> Result<std::collections::HashSet<u64>, StatusCode> {
    let Some(delivery) = delivery else {
        return Ok(std::collections::HashSet::new());
    };
    let ids = cards.iter().map(|book| book.id).collect::<Vec<_>>();
    delivery
        .available_card_ids(&ids)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

/// Map only explicitly confirmed current public covers to immutable CDN URLs.
/// Never publish/copy/remove an object during a visitor's page request.
/// Disabled by default; if the publication registry/origin was not installed,
/// keep the same-origin private-object proxy as a safe fallback.
pub(super) async fn opt_in_cdn_html(
    delivery: Option<&cover_delivery::CoverDelivery>,
    html: axum::response::Html<String>,
    ids: &[u64],
) -> Result<(axum::response::Html<String>, Option<String>), StatusCode> {
    if std::env::var("TOR_CDN_COVER_HTML_ENABLED").as_deref() != Ok("1") {
        return Ok((html, None));
    }
    let Some(delivery) = delivery else {
        return Ok((html, None));
    };
    use tor_books::repositories::cover_slots::PublicCoverKind;
    use tor_ingest::domain::cover::CoverError;
    let card = match delivery.current_cdn_urls(ids, PublicCoverKind::Card).await {
        Ok(urls) => urls,
        Err(CoverError::Forbidden) => return Ok((html, None)),
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let main = match delivery.current_cdn_urls(ids, PublicCoverKind::Main).await {
        Ok(urls) => urls,
        Err(CoverError::Forbidden) => return Ok((html, None)),
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let mut rendered = html.0;
    let mut origin: Option<String> = None;
    for (kind, urls) in [("card", card), ("main", main)] {
        for (book_id, url) in urls {
            let Some((host, _)) = url.split_once("/public/covers/") else {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            };
            if let Some(previous) = origin.as_ref() {
                if previous != host {
                    return Err(StatusCode::SERVICE_UNAVAILABLE);
                }
            } else {
                origin = Some(host.to_owned());
            }
            // Replace only server-rendered src attributes for B-approved books.
            // No user-supplied URL, book title, HTML attribute or storage key
            // controls this substitution.
            let source = format!("src=\"/covers/{book_id}/{kind}.webp\"");
            let target = format!("src=\"{url}\"");
            rendered = rendered.replace(&source, &target);
        }
    }
    Ok((axum::response::Html(rendered), origin))
}

/// Existing private-page CSP forbids third-party images; extend img-src only
/// for the verified configured CDN host, without weakening other directives.
pub(super) fn cdn_image_csp(mut response: Response, origin: Option<&str>) -> Response {
    let Some(origin) = origin else {
        return response;
    };
    if let Some(previous) = response.headers().get("content-security-policy") {
        if let Ok(policy) = previous.to_str() {
            let updated = policy.replace("img-src 'self';", &format!("img-src 'self' {origin};"));
            if let Ok(value) = axum::http::HeaderValue::from_str(&updated) {
                response
                    .headers_mut()
                    .insert("content-security-policy", value);
            }
        }
    }
    response
}

pub(super) async fn homepage_with_cdn(
    state: &AppState,
    cards: &[pages::PublicBookCard],
    updates: &[pages::PublicBookCard],
    rising: &[pages::PublicBookCard],
    covers: &std::collections::HashSet<u64>,
) -> Result<axum::response::Html<String>, StatusCode> {
    let html = if rising.is_empty() {
        if updates.is_empty() {
            pages::home_recent_with_covers(cards, covers)
        } else {
            pages::home_recent_with_updates(cards, covers, updates)
        }
    } else {
        pages::home_recent_with_growth(cards, covers, updates, rising)
    };
    let mut ids = std::collections::HashSet::new();
    for book in cards.iter().chain(updates).chain(rising) {
        ids.insert(book.id);
    }
    let ids = ids.into_iter().collect::<Vec<_>>();
    let (html, _) = opt_in_cdn_html(state.covers.as_ref(), html, &ids).await?;
    Ok(html)
}

/// Compose a verified CDN image projection with all existing book-page
/// decorations, keeping the routing shell below its architecture size limit.
pub(super) async fn book_html(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    book_id: u64,
    html: axum::response::Html<String>,
) -> Response {
    let (html, origin) = match opt_in_cdn_html(state.covers.as_ref(), html, &[book_id]).await {
        Ok(value) => value,
        Err(status) => return book_error(state, status).await,
    };
    let response = book_follow_page::attach(&state.follow_control, headers, book_id, html).await;
    crate::recommendation_http::visits::book_response(
        cdn_image_csp(response, origin.as_deref()),
        book_id,
        0,
    )
}

/// The registry must be attached deliberately; merely having an S3 adapter
/// does not activate CDN projection, and there is never a publish side effect.
pub(super) fn configured_delivery(
    catalog: &PgCatalog,
    store: Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
) -> cover_delivery::CoverDelivery {
    let enabled = std::env::var("TOR_CDN_COVER_HTML_ENABLED").as_deref() == Ok("1")
        && store.public_cdn_origin().is_some();
    let delivery = cover_delivery::CoverDelivery::new(
        tor_books::repositories::cover_slots::PgCoverSlots::new(catalog.pool()),
        store,
    );
    if enabled {
        delivery.with_publication_registry(
            tor_books::repositories::cover_publications::PgPublicCoverPublications::new(
                catalog.pool(),
            ),
        )
    } else {
        delivery
    }
}

/// No client-controlled storage paths or private S3 URLs.
pub(super) async fn cover_image(
    State(state): State<AppState>,
    Path((id, variant)): Path<(String, String)>,
) -> Response {
    let Ok(id) = id.parse::<u64>() else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    let Some(covers) = state.covers else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    let kind = match variant.as_str() {
        "card.webp" => tor_books::repositories::cover_slots::PublicCoverKind::Card,
        "main.webp" => tor_books::repositories::cover_slots::PublicCoverKind::Main,
        _ => return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
    };
    match covers.read(id, kind).await {
        Ok(Some(bytes)) => {
            auth_cookies::private_response(([(header::CONTENT_TYPE, "image/webp")], bytes), &[])
        }
        Ok(None) => auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
        Err(_) => auth_cookies::private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

#[derive(serde::Serialize)]
struct DirectCoverGrant {
    url: String,
    sha256: String,
    size_bytes: i64,
    content_type: &'static str,
    expires_in_seconds: u64,
}

/// Anonymous grants are restricted to B-approved BASIC public book covers.
/// Never expose a staging key or an unverified public object. If the direct
/// S3 origin is not configured, the endpoint behaves like an unknown URL.
pub(super) async fn cover_direct_grant(
    State(state): State<AppState>,
    Path((id, variant)): Path<(String, String)>,
) -> Response {
    let Ok(id) = id.parse::<u64>() else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    let kind = match variant.as_str() {
        "card.webp" => tor_books::repositories::cover_slots::PublicCoverKind::Card,
        "main.webp" => tor_books::repositories::cover_slots::PublicCoverKind::Main,
        _ => return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
    };
    let Some(covers) = state.covers else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    match covers.direct_grant(id, kind).await {
        Ok(Some((url, digest, size))) => {
            let sha256 = digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            auth_cookies::private_response(
                Json(DirectCoverGrant {
                    url,
                    sha256,
                    size_bytes: size,
                    content_type: "image/webp",
                    expires_in_seconds: 60,
                }),
                &[],
            )
        }
        Ok(None) => auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
        Err(tor_ingest::domain::cover::CoverError::Forbidden) => {
            auth_cookies::private_response(StatusCode::NOT_FOUND, &[])
        }
        Err(_) => auth_cookies::private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

/// Account routes retain existing ACL and CSRF checks with cover delivery enabled.
pub fn router_with_postgres_reading_auth_covers(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
    objects: Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
) -> Router {
    super::admin_reader_router::authenticated_postgres_reader_without_paid_g(
        catalog,
        reader,
        accounts,
        secure,
        public_origin,
        Some(objects),
        false,
    )
}

/// Add owner-only cover upload when a real background worker is enabled.
/// Older auth+cover routers continue to expose read-only covers.
pub fn router_with_postgres_reading_auth_cover_uploads(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
    objects: Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
) -> Router {
    super::admin_reader_router::authenticated_postgres_reader_without_paid_g(
        catalog,
        reader,
        accounts,
        secure,
        public_origin,
        Some(objects),
        true,
    )
}

/// PostgreSQL reader plus private object adapter; available in isolated tests.
pub fn router_with_postgres_reading_covers(
    catalog: PgCatalog,
    reader: PgReader,
    objects: Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
) -> Router {
    ril::instrument(make_router(
        Arc::new(catalog.clone()),
        Some(catalog),
        Some(reader),
        false,
        Some(objects),
        None,
    ))
}

#[cfg(test)]
mod cdn_html_tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn only_image_csp_is_extended() {
        let private = auth_cookies::private_response("book", &[]);
        let response = cdn_image_csp(private, Some("https://covers.example"));
        let csp = response.headers()["content-security-policy"]
            .to_str()
            .expect("valid CSP");
        assert!(csp.contains("img-src 'self' https://covers.example;"));
        assert!(csp.contains("script-src 'none'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(!csp.contains("unsafe-inline"));

        let public = "public".into_response();
        let response = cdn_image_csp(public, Some("https://covers.example"));
        assert!(response.headers().get("content-security-policy").is_none());
    }
}
