//! G-08: read-only, bounded audience enumeration for ChapterPublished.
//! No messaging, durable receipt, preference consent, or paid-content disclosure.
//! A future dispatcher must fence its lease and recheck opt-out and channel
//! consent before external delivery.

pub const RECIPIENT_BATCH_SIZE: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientPage {
    pub user_ids: Vec<i64>,
    pub next_user_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipientError {
    Invalid,
    LostLease,
    Unavailable,
}

/// Durable on-site metadata staging; not a network delivery receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagedPage {
    pub newly_staged: u64,
    pub cursor_user_id: i64,
    pub complete: bool,
}
