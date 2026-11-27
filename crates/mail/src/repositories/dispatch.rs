//! Construct the common dispatch repositories from one database pool.
//! Keep SQLx in the repository layer.
use super::{
    launch::PgMailLaunch, postbox_feedback::PgPostboxFeedback, quota::PgMailQuota, PgMailQueue,
};
use sqlx::PgPool;

pub struct DispatchStores {
    pub(crate) queue: PgMailQueue,
    pub(crate) launch: PgMailLaunch,
    pub(crate) quota: PgMailQuota,
    pub(crate) feedback: PgPostboxFeedback,
}

impl DispatchStores {
    pub fn new(pool: PgPool) -> Self {
        Self {
            queue: PgMailQueue::new(pool.clone()),
            launch: PgMailLaunch::new(pool.clone()),
            quota: PgMailQuota::new(pool.clone()),
            feedback: PgPostboxFeedback::new(pool),
        }
    }

    #[cfg(test)]
    pub(crate) fn lazy_fixture() -> Self {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://fixture@127.0.0.1:1/tor_ci")
            .unwrap();
        Self::new(pool)
    }
}
