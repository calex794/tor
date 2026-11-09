//! S349: opt-in configured free chapter writer and authenticated reading.
use super::{admin_reader_router, PgCatalog};
use axum::Router;
use std::sync::Arc;
use tor_reading::repositories::PgReader;

pub fn router_with_postgres_reading_auth_free_s3(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    origin: (bool, String),
    covers: Option<(
        Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
        bool,
    )>,
    free: crate::free_author_publish::PgFreeAuthorPublisher<
        tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
    >,
) -> Router {
    let (objects, upload) = match covers {
        Some((store, enabled)) => (Some(store), enabled),
        None => (None, false),
    };
    admin_reader_router::authenticated_postgres_reader_with_free_s3(
        catalog,
        reader,
        accounts,
        origin,
        (objects, upload),
        Arc::new(free),
    )
}
