//! Public chapter text is free-only; public book metadata can name paid chapters.
use async_trait::async_trait;
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadingError {
    Unavailable,
    InvalidPage,
}

impl fmt::Display for ReadingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for ReadingError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicChapterLink {
    pub id: u64,
    pub number: i32,
    pub title: String,
    pub is_paid: bool,
    pub price_minor: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicBookPage {
    pub id: u64,
    pub title: String,
    pub language: String,
    pub author_name: String,
    pub genres: Vec<String>,
    pub description: String,
    pub chapters: Vec<PublicChapterLink>,
    pub next_cursor: Option<String>,
}

/// Public TOC metadata for a published paid chapter; never a purchase right.
/// No chapter text, private object reference or paid edition is included.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicPaidChapterPreview {
    pub book_id: u64,
    pub book_title: String,
    pub chapter_no: i32,
    pub chapter_title: String,
    pub price_minor: i64,
}

/// Public navigation does not infer the logged-in reader's paid entitlement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicContinuation {
    ReadNow(u64),
    NeedsAccess(u64),
    WaitingForRelease,
    FinishedBook,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicChapterPage {
    pub version_id: u64,
    pub book_id: u64,
    pub book_title: String,
    pub chapter_title: String,
    pub version_no: i32,
    pub body_markdown: String,
    pub previous_chapter_id: Option<u64>,
    pub next_chapter_id: Option<u64>,
    pub next_chapter_is_paid: bool,
    /// Live lifecycle status, not inferred from the absence of the next chapter.
    pub book_completed: bool,
}

impl PublicChapterPage {
    pub fn continuation(&self) -> PublicContinuation {
        match self.next_chapter_id {
            Some(id) if self.next_chapter_is_paid => PublicContinuation::NeedsAccess(id),
            Some(id) => PublicContinuation::ReadNow(id),
            None if self.book_completed => PublicContinuation::FinishedBook,
            None => PublicContinuation::WaitingForRelease,
        }
    }
}

/// This port deliberately has no method to read paid, draft, held or deleted text.
/// Such access will need a separate authenticated policy and entitlement check.
#[async_trait]
pub trait PublicReadingRepository: Send + Sync {
    async fn public_book(&self, id: u64) -> Result<Option<PublicBookPage>, ReadingError>;
    async fn public_chapter(&self, id: u64) -> Result<Option<PublicChapterPage>, ReadingError>;
}

pub mod progress;

pub mod toc;

pub mod library;

pub mod sync;

pub mod paid_delivery;

pub mod audience;
pub mod book_returns;
pub mod group_book_returns;
pub mod notification_preferences;
pub mod notification_send_time;
pub mod returns;

#[cfg(test)]
mod ret02_continuation_tests {
    use super::{PublicChapterPage, PublicContinuation};

    fn page(next: Option<u64>, paid: bool, finished: bool) -> PublicChapterPage {
        PublicChapterPage {
            version_id: 1,
            book_id: 1,
            book_title: "Book".to_owned(),
            chapter_title: "Chapter".to_owned(),
            version_no: 1,
            body_markdown: "Text".to_owned(),
            previous_chapter_id: None,
            next_chapter_id: next,
            next_chapter_is_paid: paid,
            book_completed: finished,
        }
    }

    #[test]
    fn ret02_distinguishes_finished_book_from_unreleased_frontier() {
        assert_eq!(
            page(None, false, false).continuation(),
            PublicContinuation::WaitingForRelease
        );
        assert_eq!(
            page(None, false, true).continuation(),
            PublicContinuation::FinishedBook
        );
        assert_eq!(
            page(Some(7), false, true).continuation(),
            PublicContinuation::ReadNow(7)
        );
        assert_eq!(
            page(Some(8), true, true).continuation(),
            PublicContinuation::NeedsAccess(8)
        );
    }
}
