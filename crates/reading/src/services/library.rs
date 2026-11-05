//! C-08/S007: saved book links are independent from reading progress and purchases.
#![forbid(unsafe_code)]

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
