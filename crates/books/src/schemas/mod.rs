use serde::Serialize;
#[derive(Serialize)]
pub struct BookView {
    pub id: u64,
    pub title: String,
    pub target_language: String,
}

/// Author-visible first-release schedule, independent of database storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledChapter {
    pub scheduled_utc: String,
    pub status: String,
}
