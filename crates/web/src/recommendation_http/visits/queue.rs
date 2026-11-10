//! Bounded best-effort capture. Retries reuse the same event receipt.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::sync::mpsc;
use tor_recommendations::{
    repositories::visits::PgFlowVisits,
    services::visits::{FlowVisitError, VisitObservation},
};

static ACTIVE: AtomicU64 = AtomicU64::new(0);
static OFFERED: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static SCOPE_TIMEOUTS: AtomicU64 = AtomicU64::new(0);
static SAVED: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static FAILURES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub(crate) struct Snapshot {
    pub active: bool,
    pub offered: u64,
    pub dropped: u64,
    pub scope_timeouts: u64,
    pub saved: u64,
    pub skipped: u64,
    pub failures: u64,
}

pub(crate) fn snapshot() -> Snapshot {
    Snapshot {
        active: ACTIVE.load(Ordering::Relaxed) > 0,
        offered: OFFERED.load(Ordering::Relaxed),
        dropped: DROPPED.load(Ordering::Relaxed),
        scope_timeouts: SCOPE_TIMEOUTS.load(Ordering::Relaxed),
        saved: SAVED.load(Ordering::Relaxed),
        skipped: SKIPPED.load(Ordering::Relaxed),
        failures: FAILURES.load(Ordering::Relaxed),
    }
}

#[derive(Clone)]
pub(super) struct Queue(mpsc::Sender<VisitObservation>);

impl Queue {
    pub(super) fn start(store: PgFlowVisits) -> Option<Self> {
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let (tx, mut rx) = mpsc::channel::<VisitObservation>(512);
        ACTIVE.fetch_add(1, Ordering::Relaxed);
        let active = Active;
        runtime.spawn(async move {
            let _active = active;
            while let Some(observation) = rx.recv().await {
                let mut completed = false;
                for attempt in 0..3 {
                    let result = tokio::time::timeout(
                        Duration::from_millis(750),
                        store.observe(&observation),
                    )
                    .await;
                    if let Ok(Ok(saved)) = result {
                        if saved {
                            SAVED.fetch_add(1, Ordering::Relaxed);
                        } else {
                            SKIPPED.fetch_add(1, Ordering::Relaxed);
                        }
                        completed = true;
                        break;
                    }
                    FAILURES.fetch_add(1, Ordering::Relaxed);
                    if matches!(
                        result,
                        Ok(Err(FlowVisitError::Invalid | FlowVisitError::Conflict))
                    ) {
                        break;
                    }
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
                if !completed {
                    DROPPED.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        Some(Self(tx))
    }

    pub(super) fn offer(&self, observation: VisitObservation) {
        OFFERED.fetch_add(1, Ordering::Relaxed);
        if self.0.try_send(observation).is_err() {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn scope_timeout(&self) {
        SCOPE_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
    }
}

struct Active;
impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}
