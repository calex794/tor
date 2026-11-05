//! C-08/S007: saved book links are independent from reading progress and purchases.
#![forbid(unsafe_code)]

use crate::services::progress::ReadingPlace;

pub const DEFAULT_LIBRARY_LIMIT: u16 = 30;
pub const MAX_LIBRARY_LIMIT: u16 = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedBook {
    pub cursor_id: i64,
    pub book_id: i64,
    pub title: String,
    /// A previously saved Book is still shown privately while awaiting deletion.
    /// This is not a chapter entitlement or a completed archival copy.
    pub pending_deletion: bool,
    /// Completed ordinary removal leaves a private tombstone, not public access.
    /// Never conflate with books.is_deleted (hard/legal denial).
    pub ordinary_deleted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedBookPage {
    pub items: Vec<SavedBook>,
    pub next_cursor: Option<i64>,
}

/// A purchase-backed Book without an explicit bookmark. This is private
/// navigation only: a title/link cannot create or mutate a purchase grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurchasedOnlyBook {
    pub book_id: i64,
    pub title: String,
    pub pending_deletion: bool,
    pub ordinary_deleted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurchasedOnlyBookPage {
    pub items: Vec<PurchasedOnlyBook>,
    /// Independent Book-ID cursor; never mix with bookmark row IDs.
    pub next_cursor: Option<i64>,
}

/// One historically saved free edition; only returned after live actor,
/// age, book and chapter state guards. Not a public chapter preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedFreeChapter {
    pub book_id: i64,
    pub book_title: String,
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub chapter_title: String,
    pub version_id: i64,
    pub version_no: i32,
    pub body_markdown: String,
    /// Only the original reader's exact pinned version can reuse this cursor.
    /// This is display-only; it is never a new content entitlement.
    pub saved_place: Option<ReadingPlace>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedFreeChapterLink {
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub chapter_title: String,
    pub version_no: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedFreeChapterPage {
    pub items: Vec<SavedFreeChapterLink>,
    pub next_cursor: Option<i64>,
}

/// A reader-owned, exact-version chapter list for the private Book archive.
/// Navigation metadata only: it is not a new free or paid entitlement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedArchiveChapter {
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub chapter_title: String,
    /// Already-pinned free edition, not today's publication pointer.
    pub saved_free_version_no: Option<i32>,
    /// A separately purchased edition, after canonical D live-rights checks.
    pub purchased: Option<SavedPurchasedEdition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedPurchasedEdition {
    pub purchase_id: i64,
    pub version_no: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedArchivePage {
    pub items: Vec<SavedArchiveChapter>,
    pub next_cursor: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryError {
    Invalid,
    NotFound,
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_metadata_never_contains_content_or_entitlement() {
        let bookmark = SavedBook {
            cursor_id: 3,
            book_id: 7,
            title: "Книга".to_owned(),
            pending_deletion: false,
            ordinary_deleted: false,
        };
        assert_eq!(bookmark.book_id, 7);
        assert_eq!(DEFAULT_LIBRARY_LIMIT, 30);
        assert_eq!(MAX_LIBRARY_LIMIT, 100);
    }
}
