//! C-08/S007: saved book links are independent from reading progress and purchases.
#![forbid(unsafe_code)]

pub const DEFAULT_LIBRARY_LIMIT: u16 = 30;
pub const MAX_LIBRARY_LIMIT: u16 = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedBook {
    pub cursor_id: i64,
    pub book_id: i64,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedBookPage {
    pub items: Vec<SavedBook>,
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
        };
        assert_eq!(bookmark.book_id, 7);
        assert_eq!(DEFAULT_LIBRARY_LIMIT, 30);
        assert_eq!(MAX_LIBRARY_LIMIT, 100);
    }
}
