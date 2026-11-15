//! Functional C repositories do not collect optional audience identities.
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
    Stores {
        progress: PgReadingProgress::new(pool.clone()),
        sync: PgReadingSync::new(pool.clone()),
        bookmarks: PgLibrary::new(pool.clone()),
        book_follows: PgBookFollows::new(pool.clone()),
    }
}
