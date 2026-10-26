//! Only integration crate may wire G cover worker to B's authoritative slot.
//! Neither tor-books nor tor-ingest depends directly on the other feature.
use async_trait::async_trait;
use tor_books::repositories::cover_slots::PgCoverSlots;
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::BookCoverAttach};

#[derive(Clone)]
pub struct PgBookCoverBridge {
    slots: PgCoverSlots,
}
impl PgBookCoverBridge {
    pub fn new(slots: PgCoverSlots) -> Self {
        Self { slots }
    }
}

#[async_trait]
impl BookCoverAttach for PgBookCoverBridge {
    async fn attach_ready(&self, job_id: i64, epoch: i64) -> Result<bool, CoverError> {
        self.slots
            .attach_ready_job(job_id, epoch)
            .await
            .map_err(|_| CoverError::Unavailable)
    }
}
