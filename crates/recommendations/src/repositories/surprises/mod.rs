//! F-SURPRISE: account-owned, durable, non-monetary surprise decisions.
use sqlx::PgPool;

pub mod admin;
mod content;
mod decisions;
pub mod experiment;
mod maintenance;

#[derive(Clone)]
pub struct PgSurprises {
    pub(super) pool: PgPool,
}

impl PgSurprises {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurpriseError {
    Forbidden,
    Invalid,
    Unavailable,
}

impl From<sqlx::Error> for SurpriseError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanState {
    Off,
    NotSelected,
    Planned {
        utc_day: i64,
        active_after_seconds: i32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurpriseContent {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub book_id: Option<i64>,
    pub editorial: bool,
}
