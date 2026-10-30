//! SQLx adapters owned by G.
pub mod cover_jobs;
pub mod notify;
pub mod outbox;

#[cfg(feature = "s3")]
pub mod s3;
