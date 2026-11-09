//! S349 free author variants. A configured S3 writer is required for Save.
use super::{router_inner, CoverObjectStore, PgAccounts, PgReader, Router};
use crate::author_chapter_http::{FreeChapterPublisher, PaidChapterPublisher};
use std::sync::Arc;

pub(crate) fn router_with_free_author(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    cover: Option<Arc<dyn CoverObjectStore>>,
    free: Arc<dyn FreeChapterPublisher>,
) -> Router {
    router_inner(
        accounts,
        reader,
        secure,
        origin,
        cover,
        false,
        (None, Some(free)),
    )
}

pub(crate) fn router_with_paid_reader_free_author(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    cover: Option<Arc<dyn CoverObjectStore>>,
    free: Arc<dyn FreeChapterPublisher>,
) -> Router {
    router_inner(
        accounts,
        reader,
        secure,
        origin,
        cover,
        true,
        (None, Some(free)),
    )
}

pub(crate) fn router_with_paid_and_free_author(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    cover: Option<Arc<dyn CoverObjectStore>>,
    paid: Arc<dyn PaidChapterPublisher>,
    free: Arc<dyn FreeChapterPublisher>,
) -> Router {
    router_inner(
        accounts,
        reader,
        secure,
        origin,
        cover,
        true,
        (Some(paid), Some(free)),
    )
}
