//! Data shown under the current published free chapter edition.
//! Storage checks edition, visibility and permissions before producing this view.

/// Public view of a position-bound comment on the current free edition.
/// No reader identity or paid entitlement is present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChapterCommentAnchor {
    pub block_index: i32,
    pub start_utf16: i32,
    pub end_utf16: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChapterComment {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub body: String,
    pub anchor: Option<ChapterCommentAnchor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChapterCommentPage {
    pub items: Vec<ChapterComment>,
    pub next_cursor: Option<i64>,
}
