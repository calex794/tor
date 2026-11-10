//! Explicit process-level collection gates; repositories remain off by default.
use tor_accounts::repositories::PgAccounts;
use tor_reading::repositories::{PgBookFollows, PgLibrary, PgReadingProgress, PgReadingSync};

pub(super) struct Stores {
    pub progress: PgReadingProgress,
    pub sync: PgReadingSync,
    pub bookmarks: PgLibrary,
    pub book_follows: PgBookFollows,
}

pub(super) fn stores(accounts: &PgAccounts) -> Stores {
    let pool = accounts.pool();
    let activity = crate::recommendation_worker::reading_returns_enabled();
    let book_returns = crate::recommendation_worker::book_reading_returns_enabled();
    let flow = crate::recommendation_http::visits::collection_enabled();
    Stores {
        progress: PgReadingProgress::new(pool.clone())
            .with_activity_collection(activity)
            .with_book_return_collection(book_returns)
            .with_flow_collection(flow),
        sync: PgReadingSync::new(pool.clone())
            .with_activity_collection(activity)
            .with_book_return_collection(book_returns)
            .with_flow_collection(flow),
        bookmarks: PgLibrary::new(pool.clone()).with_flow_collection(flow),
        book_follows: PgBookFollows::new(pool.clone()).with_flow_collection(flow),
    }
}
