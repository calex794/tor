//! Thin HTTP adapter and optional router constructors for private book covers.
use super::{auth, auth_cookies, cover_delivery, make_router, pages, ril, AppState};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::Response,
    Router,
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

/// Account routes retain existing ACL and CSRF checks with cover delivery enabled.
pub fn router_with_postgres_reading_auth_covers(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
    objects: Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
) -> Router {
    let auth_reader = reader.clone();
    ril::instrument(
        make_router(
            Arc::new(catalog.clone()),
            Some(catalog),
            Some(reader),
            true,
            Some(objects),
        )
        .merge(auth::router(accounts, auth_reader, secure, public_origin)),
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
    ))
}
