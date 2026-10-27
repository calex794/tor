//! S034 grant/paid reader router integration; keeps the web shell compact.
use super::{auth, auth_cookies, make_router, paid_grants_http, ril};
use axum::Router;
use std::sync::Arc;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;

/// S034 opt-in: the grant API remains independent from the browser reader.
/// Other demo/PG constructors remain default-off.
pub fn router_with_postgres_reading_auth_paid_grants(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
    paid_storage: Arc<dyn tor_contracts::paid_delivery::PrivateObjectDeliveryPort>,
    cover: Option<(
        Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
        bool,
    )>,
) -> Router {
    paid_grants_router(
        catalog,
        reader,
        accounts,
        secure,
        public_origin,
        (paid_storage, None),
        cover,
    )
    .expect("API-only grant constructor has no browser S3 origin")
}

/// Enable the actual H reader only with an explicitly validated S3 origin.
/// It shares the same rights and signed-grant route as the API-only variant.
pub fn router_with_postgres_reading_auth_paid_grants_browser(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
    paid: (
        Arc<dyn tor_contracts::paid_delivery::PrivateObjectDeliveryPort>,
        String,
    ),
    cover: Option<(
        Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
        bool,
    )>,
) -> Result<Router, &'static str> {
    paid_grants_router(
        catalog,
        reader,
        accounts,
        secure,
        public_origin,
        (paid.0, Some(paid.1)),
        cover,
    )
}

fn paid_grants_router(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
    paid: (
        Arc<dyn tor_contracts::paid_delivery::PrivateObjectDeliveryPort>,
        Option<String>,
    ),
    cover: Option<(
        Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
        bool,
    )>,
) -> Result<Router, &'static str> {
    let (cover_storage, enable_cover_uploads) = match cover {
        Some((store, enabled)) => (Some(store), enabled),
        None => (None, false),
    };
    let auth_reader = reader.clone();
    let cookies = auth_cookies::CookiePolicy {
        secure,
        origin: public_origin.clone(),
    };
    let grants = if let Some(s3_origin) = paid.1 {
        paid_grants_http::router_with_reader(accounts.clone(), cookies, paid.0, s3_origin)?
    } else {
        paid_grants_http::router(accounts.clone(), cookies, paid.0)
    };
    let author = if enable_cover_uploads {
        match cover_storage.as_ref() {
            Some(store) => auth::router_with_cover_uploads(
                accounts,
                auth_reader,
                secure,
                public_origin,
                store.clone(),
            ),
            None => auth::router(accounts, auth_reader, secure, public_origin),
        }
    } else {
        auth::router(accounts, auth_reader, secure, public_origin)
    };
    Ok(ril::instrument(
        make_router(
            Arc::new(catalog.clone()),
            Some(catalog),
            Some(reader),
            true,
            cover_storage,
        )
        .merge(author)
        .merge(grants),
    ))
}
