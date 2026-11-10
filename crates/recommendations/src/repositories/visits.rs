//! Optional, consent-scoped visit facts. The HTTP layer only enqueues them.
use crate::services::visits::FlowVisitError;
use sqlx::PgPool;

mod book;
mod maintenance;
mod observe;
mod report;

#[derive(Clone)]
pub struct PgFlowVisits {
    pub(super) pool: PgPool,
}

impl PgFlowVisits {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

pub(super) fn unavailable(_: sqlx::Error) -> FlowVisitError {
    FlowVisitError::Unavailable
}
