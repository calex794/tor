//! Anonymous per-card observation wiring. Issuance never blocks an HTML response.
use crate::auth_cookies::CookiePolicy;
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
    Extension, Router,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::mpsc;
use tor_accounts::repositories::PgAccounts;
use tor_contracts::exposure::{CardObservation, EditorialObservationGuard, ObservationIssuer};
use tor_recommendations::{
    domain::exposure::IssuedObservation, repositories::exposure::PgExposure,
};
mod bootstrap;
mod client;
mod ingest;
mod render;
mod report;
mod worker;
pub use bootstrap::start;
pub use worker::Worker;

#[derive(Default)]
struct Counters {
    queued: AtomicU64,
    dropped: AtomicU64,
    persisted: AtomicU64,
    failed: AtomicU64,
}
#[derive(Clone, Copy, Debug)]
pub struct QueueSnapshot {
    pub queued: u64,
    pub dropped: u64,
    pub persisted: u64,
    pub failed: u64,
}
impl Counters {
    fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            queued: self.queued.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            persisted: self.persisted.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }
}
struct Issuer {
    sender: mpsc::Sender<IssuedObservation>,
    counters: Arc<Counters>,
}
impl ObservationIssuer for Issuer {
    fn issue(&self, card: CardObservation) -> Option<String> {
        let (proof, item) = tor_recommendations::services::exposure::issue(card).ok()?;
        match self.sender.try_send(item) {
            Ok(()) => {
                self.counters.queued.fetch_add(1, Ordering::Relaxed);
                Some(proof)
            }
            Err(_) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
}
#[derive(Clone)]
pub struct Runtime {
    accounts: PgAccounts,
    cookies: CookiePolicy,
    store: PgExposure,
    issuer: Arc<dyn ObservationIssuer>,
    counters: Arc<Counters>,
    guard: Option<Arc<dyn EditorialObservationGuard>>,
}
impl Runtime {
    /// Construction does not spawn tasks. Start the returned worker once in main/tests.
    pub fn new(
        accounts: PgAccounts,
        origin: String,
        secure: bool,
        guard: Option<Arc<dyn EditorialObservationGuard>>,
    ) -> (Self, Worker) {
        let store = PgExposure::new(accounts.pool().clone());
        let (sender, receiver) = mpsc::channel(1024);
        let counters = Arc::new(Counters::default());
        let issuer = Arc::new(Issuer {
            sender,
            counters: counters.clone(),
        });
        (
            Self {
                accounts,
                cookies: CookiePolicy { origin, secure },
                store: store.clone(),
                issuer,
                counters: counters.clone(),
                guard,
            },
            Worker {
                store,
                receiver,
                counters,
            },
        )
    }
    pub fn issuer(&self) -> Arc<dyn ObservationIssuer> {
        self.issuer.clone()
    }
    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.counters.snapshot()
    }
    pub fn attach(self, app: Router) -> Router {
        let issuer = self.issuer();
        app.merge(crate::ril::instrument_additional(
            Router::new()
                .route(
                    "/api/v1/flow/observations",
                    post(ingest::event).layer(DefaultBodyLimit::max(4096)),
                )
                .route("/assets/flow-observations.js", get(client::script))
                .route("/admin/audience/exposure", get(report::page))
                .with_state(self),
        ))
        .layer(middleware::from_fn(client::append_script))
        .layer(Extension(issuer))
    }
}

/// Called only inside trusted Rust renderers, never by parsing author HTML.
pub(crate) fn attributes(issuer: Option<&dyn ObservationIssuer>, card: CardObservation) -> String {
    issuer
        .and_then(|issuer| issuer.issue(card))
        .filter(|proof| crate::auth_cookies::valid_token(proof))
        .map(|proof| format!(" data-flow-proof=\"{proof}\""))
        .unwrap_or_default()
}
