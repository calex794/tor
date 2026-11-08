//! Types of private purchased chapter navigation exposed to web pages.
//! These are metadata only: no chapter body, signed URLs or purchase grant.

/// An owned, readable edition. Never contains paid text or signed object URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadablePurchase {
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
