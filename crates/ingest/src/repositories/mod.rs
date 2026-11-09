//! SQLx adapters owned by G.
pub mod cover_jobs;
pub mod outbox;
pub mod outbox_retry;

#[cfg(feature = "s3")]
pub mod s3;
