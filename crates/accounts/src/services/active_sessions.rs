//! Account-owned view of active sessions, without token hashes or raw IP data.
//!
//! This is a display record; revocation and ownership checks remain in PostgreSQL.

pub struct ActiveSession {
    pub id: String,
    pub opened_utc: String,
    pub browser_family: String,
    pub current: bool,
}
