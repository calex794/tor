//! RIL-10 read-only SQLx completion observer: capture ONLY numeric elapsed_secs.
//! The tracing layer never reads/logs SQL text, parameters, raw URLs or identities.
//! Scope is Tokio task-local: spawned tasks are not counted, nor claimed covered.
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
};
use tracing::{
    field::{Field, Visit},
    Event, Metadata, Subscriber,
};
use tracing_subscriber::{
    layer::{Context, SubscriberExt},
    util::SubscriberInitExt,
    Layer,
};

static INSTALLED: OnceLock<bool> = OnceLock::new();
const MAX_US_PER_REQUEST: u64 = 1_000_000_000;
tokio::task_local! {
    static QUERY_COUNTER: Arc<QueryCounter>;
}

#[derive(Default)]
struct QueryCounter {
    completed: AtomicU64,
    elapsed_us: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SqlxMeasurement {
    pub(crate) queries: u64,
    pub(crate) elapsed_us: u64,
}

struct ElapsedVisitor {
    seconds: Option<f64>,
}

impl Visit for ElapsedVisitor {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.seconds = Some(value);
        }
    }
}

/// Only accepts SQLx target metadata. No formatting/logging layer is installed.
struct SqlxCompletionLayer;

impl<S: Subscriber> Layer<S> for SqlxCompletionLayer {
    fn enabled(&self, meta: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        meta.is_event() && meta.target() == "sqlx::query"
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut visitor = ElapsedVisitor { seconds: None };
        event.record(&mut visitor);
        if let Some(seconds) = visitor.seconds.filter(|v| v.is_finite() && *v >= 0.0) {
            // QueryLogger reports client elapsed time, including transport and
            // response consumption. It is NOT PG server CPU/execution time.
            let elapsed = (seconds * 1_000_000.0).round() as u64;
            let _ = QUERY_COUNTER.try_with(|counter| {
                let _ = counter
                    .completed
                    .try_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                        old.checked_add(1)
                            .filter(|value| *value <= MAX_US_PER_REQUEST)
                    });
                let _ =
                    counter
                        .elapsed_us
                        .try_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                            old.checked_add(elapsed)
                                .filter(|value| *value <= MAX_US_PER_REQUEST)
                        });
            });
        }
    }
}

/// Safe to call only once during production startup, *before* PG connections.
/// If another subscriber already exists, identified SQL measurement stays OFF.
/// No log formatting subscriber is installed, so statements never print.
pub(crate) fn install_for_process() -> bool {
    *INSTALLED.get_or_init(|| {
        tracing_subscriber::registry()
            .with(SqlxCompletionLayer)
            .try_init()
            .is_ok()
    })
}

pub(crate) async fn observe<F: Future>(future: F) -> (F::Output, Option<SqlxMeasurement>) {
    if !INSTALLED.get().copied().unwrap_or(false) {
        return (future.await, None);
    }
    let scope = Arc::new(QueryCounter::default());
    let output = QUERY_COUNTER.scope(scope.clone(), future).await;
    let measurement = SqlxMeasurement {
        queries: scope.completed.load(Ordering::Relaxed),
        elapsed_us: scope.elapsed_us.load(Ordering::Relaxed),
    };
    (output, Some(measurement))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scoped_completion_events_have_no_leaked_identity_or_cross_request_state() {
        assert!(install_for_process());
        let ((), first) = observe(async {
            tracing::event!(
                target: "sqlx::query",
                tracing::Level::DEBUG,
                elapsed_secs = 0.0025_f64,
                db.statement = "SECRET FROM SQL MUST NEVER LEAVE THE LAYER"
            );
        })
        .await;
        assert_eq!(
            first,
            Some(SqlxMeasurement {
                queries: 1,
                elapsed_us: 2500
            })
        );
        let ((), second) = observe(async {}).await;
        assert_eq!(
            second,
            Some(SqlxMeasurement {
                queries: 0,
                elapsed_us: 0
            })
        );
    }

    #[tokio::test]
    async fn non_sqlx_events_are_ignored_even_in_an_active_request() {
        assert!(install_for_process());
        let ((), observed) = observe(async {
            tracing::event!(target: "tor::web", tracing::Level::DEBUG, elapsed_secs = 3.0_f64);
        })
        .await;
        assert_eq!(observed.unwrap().queries, 0);
    }
}
