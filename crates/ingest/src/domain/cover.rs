//! Media ingestion: provenance and fencing values never come from client JSON.
#[derive(Debug, Clone)]
pub struct CoverClaim {
    pub job_id: i64,
    pub book_id: i64,
    pub actor_user_id: i64,
    pub source_key: String,
    pub source_mime: String,
    pub source_size: i64,
    pub source_sha256: [u8; 32],
    pub epoch: i64,
    pub recipe_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverVariant {
    pub name: String,
    pub storage_key: String,
    pub sha256: [u8; 32],
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CoverError {
    Invalid,
    Forbidden,
    Conflict,
    LostLease,
    Unavailable,
}
impl std::fmt::Display for CoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CoverError {}
