//! Durable file-import job state returned by the G-owned repository.
#[derive(Debug, Clone)]
pub struct ChapterImportClaim {
    pub job_id: i64,
    pub book_id: i64,
    pub actor_id: i64,
    pub command_key: String,
    pub source_key: String,
    pub source_filename: String,
    pub source_sha256: Vec<u8>,
    pub source_bytes: i64,
    pub approved_fingerprint: Vec<u8>,
    pub expected_chapter_count: i32,
    pub attempts: i32,
    pub epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterImportStatus {
    pub status: String,
    pub attempts: i32,
    pub saved_count: i32,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportJobsError {
    Invalid,
    Conflict,
    Missing,
    Unavailable,
}
