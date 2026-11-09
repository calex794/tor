//! Site-administrator current unsealed reading in normal PG/auth modes.
//! This does not depend on a private G adapter or enable paid checkout.
use super::{
    auth, auth_cookies, book_follow_page, make_router_with_paid_checkout, paid_grants_http, ril,
    ReaderModes,
};
use axum::Router;
use std::sync::Arc;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;

/// Authenticated PostgreSQL without private paid G. The administrator's
/// current plaintext draft can be read, while paid S3/checkout routes
/// remain absent. The same constructor is used by cover-only modes.
pub(super) fn authenticated_postgres_reader_without_paid_g(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
    cover_objects: Option<Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>>,
    enable_cover_uploads: bool,
) -> Router {
    let auth_reader = reader.clone();
    let follows =
        book_follow_page::FollowBookControl::new(accounts.clone(), secure, public_origin.clone());
    let admin = paid_grants_http::administrator_only_router(
        accounts.clone(),
        auth_cookies::CookiePolicy {
            secure,
            origin: public_origin.clone(),
        },
    );
    let owner_auth = match (enable_cover_uploads, cover_objects.as_ref()) {
        (true, Some(store)) => auth::router_with_cover_uploads(
            accounts,
            auth_reader,
            secure,
            public_origin,
            store.clone(),
        ),
        _ => auth::router(accounts, auth_reader, secure, public_origin),
    };
    ril::instrument(
        make_router_with_paid_checkout(
            Arc::new(catalog.clone()),
            Some(catalog),
            Some(reader),
            true,
            cover_objects,
            Some(follows),
            ReaderModes {
                paid_browser: false,
                admin_reader: true,
            },
        )
        .merge(owner_auth)
        .merge(admin),
    )
}

/// Fresh-install S349: free chapter writes are never allowed to fall back to PG.
#[cfg(feature = "s3-free")]
pub(super) fn authenticated_postgres_reader_with_free_s3(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    origin: (bool, String),
    covers: (
        Option<Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>>,
        bool,
    ),
    free: Arc<dyn crate::author_chapter_http::FreeChapterPublisher>,
) -> Router {
    let (secure, public_origin) = origin;
    let (cover_objects, enable_cover_uploads) = covers;
    let auth_reader = reader.clone();
    let follows =
        book_follow_page::FollowBookControl::new(accounts.clone(), secure, public_origin.clone());
    let admin = paid_grants_http::administrator_only_router(
        accounts.clone(),
        auth_cookies::CookiePolicy {
            secure,
            origin: public_origin.clone(),
        },
    );
    let cover = if enable_cover_uploads {
        cover_objects.clone()
    } else {
        None
    };
    let owner_auth =
        auth::router_with_free_author(accounts, auth_reader, secure, public_origin, cover, free);
    ril::instrument(
        make_router_with_paid_checkout(
            Arc::new(catalog.clone()),
            Some(catalog),
            Some(reader),
            true,
            cover_objects,
            Some(follows),
            ReaderModes {
                paid_browser: false,
                admin_reader: true,
            },
        )
        .merge(owner_auth)
        .merge(admin),
    )
}
