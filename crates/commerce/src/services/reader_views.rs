//! Types of private purchased chapter navigation exposed to web pages.
//! These are metadata only: no chapter body, signed URLs or purchase grant.

/// An owned, readable edition. Never contains paid text or signed object URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadablePurchase {
    pub book_id: i64,
    pub purchase_id: i64,
    pub chapter_id: i64,
    pub version_id: i64,
    pub version_no: i32,
    pub chapter_no: i32,
    pub book_title: String,
    pub chapter_title: String,
}

/// D-authorized navigation link, not a new purchase grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadablePurchaseLink {
    pub chapter_id: i64,
    pub purchase_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadablePurchasesPage {
    pub items: Vec<ReadablePurchase>,
    pub next_after: Option<i64>,
}

/// Bounded navigation for versions actually owned in this same book.
/// A public TOC is deliberately not used to infer paid access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadableBookToc {
    pub items: Vec<ReadablePurchase>,
    pub truncated: bool,
}

/// A private reader's combined TOC. Only confirmed historical purchases
/// and separately public chapter metadata can enter this result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixedTocAccess {
    Purchased {
        purchase_id: i64,
        version_id: i64,
        version_no: i32,
    },
    PublicFree,
    Locked {
        price_minor: Option<i64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedTocChapter {
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub title: String,
    pub access: MixedTocAccess,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedBookToc {
    pub chapters: Vec<MixedTocChapter>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixedTocCursor {
    pub chapter_no: i32,
    pub chapter_id: i64,
}

impl MixedTocCursor {
    pub fn parse(raw: &str) -> Option<Self> {
        let (number, id) = raw.split_once('.')?;
        if number.starts_with('0')
            || id.starts_with('0')
            || !number.bytes().all(|ch| ch.is_ascii_digit())
            || !id.bytes().all(|ch| ch.is_ascii_digit())
        {
            return None;
        }
        let number = number.parse::<i32>().ok()?;
        let id = id.parse::<i64>().ok()?;
        (number > 0 && id > 0).then_some(Self {
            chapter_no: number,
            chapter_id: id,
        })
    }

    pub fn encode(self) -> String {
        format!("{}.{}", self.chapter_no, self.chapter_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixedTocSide {
    Before,
    After,
}

/// A paged D-scoped chapter projection; cursors are never purchase grants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedTocWindow {
    pub chapters: Vec<MixedTocChapter>,
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod toc_window_tests {
    use super::MixedTocCursor;
    #[test]
    fn cursor_is_positive_canonical_pair() {
        assert_eq!(
            MixedTocCursor::parse("1357.81741"),
            Some(MixedTocCursor {
                chapter_no: 1357,
                chapter_id: 81741,
            })
        );
        for invalid in ["", "0.1", "1.0", "01.1", "-1.8", "1.x", "1.2.3", "1. 4"] {
            assert!(MixedTocCursor::parse(invalid).is_none(), "{invalid}");
        }
    }
}

/// D-05 Layer CURRENT navigation. Metadata is not a purchase grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerTocAnchor {
    pub book_id: i64,
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub chapter_title: String,
    pub book_title: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerTocAccess {
    Subscription,
    PublicFree,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerTocChapter {
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub title: String,
    pub access: LayerTocAccess,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerTocWindow {
    pub chapters: Vec<LayerTocChapter>,
    pub next_cursor: Option<String>,
}
