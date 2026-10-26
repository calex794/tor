//! RIL-10 numeric-only SQLx completion observer.
//! Never collect query text, parameters, raw URLs, IPs or account identities.
//! Scope is one Tokio request task; spawned tasks are deliberately unmeasured.
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Instant,
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
pub(crate) const MAX_SQL_SAMPLES: usize = 8;
tokio::task_local! {
    static QUERY_COUNTER: Arc<QueryCounter>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuerySample {
    pub(crate) completed_at_us: u64,
    pub(crate) elapsed_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct QueryBuffer {
    samples: [QuerySample; MAX_SQL_SAMPLES],
    count: u8,
    uncaptured: u64,
}

struct QueryCounter {
    started: Instant,
    completed: AtomicU64,
    elapsed_us: AtomicU64,
    samples: Mutex<QueryBuffer>,
}

impl QueryCounter {
    fn new(started: Instant) -> Self {
        Self {
            started,
            completed: AtomicU64::new(0),
            elapsed_us: AtomicU64::new(0),
            samples: Mutex::new(QueryBuffer::default()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SqlxMeasurement {
    pub(crate) queries: u64,
    pub(crate) elapsed_us: u64,
    pub(crate) samples: [QuerySample; MAX_SQL_SAMPLES],
    pub(crate) captured: u8,
    pub(crate) uncaptured: u64,
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

/// Only accepts SQLx numeric timing metadata; no formatting subscriber.
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
            // SQLx client timing includes transport and response consumption,
            // and must NOT be mistaken for PostgreSQL server execution time.
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
                let mut samples = counter
                    .samples
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if elapsed > MAX_US_PER_REQUEST || usize::from(samples.count) >= MAX_SQL_SAMPLES {
                    samples.uncaptured = samples.uncaptured.saturating_add(1);
                } else {
                    let index = usize::from(samples.count);
                    samples.samples[index] = QuerySample {
                        completed_at_us: counter
                            .started
                            .elapsed()
                            .as_micros()
                            .min(u64::MAX as u128) as u64,
                        elapsed_us: elapsed,
                    };
                    samples.count += 1;
                }
            });
        }
    }
}

/// Install only once before connecting to PostgreSQL. If a different global
/// subscriber already exists, measurement stays OFF and is reported as unknown.
pub(crate) fn install_for_process() -> bool {
    *INSTALLED.get_or_init(|| {
        tracing_subscriber::registry()
            .with(SqlxCompletionLayer)
            .try_init()
            .is_ok()
    })
}

/// `started` is the HTTP handler's start, shared with the root request trace.
pub(crate) async fn observe<F: Future>(
    future: F,
    started: Instant,
) -> (F::Output, Option<SqlxMeasurement>) {
    if !INSTALLED.get().copied().unwrap_or(false) {
        return (future.await, None);
    }
    let scope = Arc::new(QueryCounter::new(started));
    let output = QUERY_COUNTER.scope(scope.clone(), future).await;
    let captured = *scope
        .samples
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let measurement = SqlxMeasurement {
        queries: scope.completed.load(Ordering::Relaxed),
        elapsed_us: scope.elapsed_us.load(Ordering::Relaxed),
        samples: captured.samples,
        captured: captured.count,
        uncaptured: captured.uncaptured,
    };
    (output, Some(measurement))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scoped_events_keep_numeric_spans_without_query_text() {
        assert!(install_for_process());
        let ((), first) = observe(
            async {
                tracing::event!(
                    target: "sqlx::query",
                    tracing::Level::DEBUG,
                    elapsed_secs = 0.0025_f64,
                    db.statement = "SECRET FROM SQL MUST NEVER LEAVE THE LAYER"
                );
            },
            Instant::now(),
        )
        .await;
        let measured = first.unwrap();
        assert_eq!(measured.queries, 1);
        assert_eq!(measured.elapsed_us, 2500);
        assert_eq!(measured.captured, 1);
        assert_eq!(measured.samples[0].elapsed_us, 2500);
        assert_eq!(measured.uncaptured, 0);
        let ((), second) = observe(async {}, Instant::now()).await;
        assert_eq!(second, Some(SqlxMeasurement::default()));
    }

    #[tokio::test]
    async fn non_sqlx_events_are_ignored() {
        assert!(install_for_process());
        let ((), observed) = observe(
            async {
                tracing::event!(target: "tor::web", tracing::Level::DEBUG, elapsed_secs = 3.0_f64);
            },
            Instant::now(),
        )
        .await;
        assert_eq!(observed, Some(SqlxMeasurement::default()));
    }

    #[tokio::test]
    async fn concurrent_requests_have_isolated_query_samples() {
        assert!(install_for_process());
        let (((), a), ((), b), ((), c)) = tokio::join!(
            observe(
                async {
                    tracing::event!(
                        target: "sqlx::query",
                        tracing::Level::DEBUG,
                        elapsed_secs = 0.001_f64
                    );
                },
                Instant::now(),
            ),
            observe(
                async {
                    for _ in 0..2 {
                        tracing::event!(
                            target: "sqlx::query",
                            tracing::Level::DEBUG,
                            elapsed_secs = 0.002_f64
                        );
                    }
                },
                Instant::now(),
            ),
            observe(async {}, Instant::now()),
        );
        assert_eq!(a.unwrap().queries, 1);
        assert_eq!(b.unwrap().queries, 2);
        assert_eq!(c.unwrap().queries, 0);
        assert_eq!(a.unwrap().captured, 1);
        assert_eq!(b.unwrap().captured, 2);
        assert_eq!(c.unwrap().captured, 0);
    }

    #[tokio::test]
    async fn sampled_spans_are_bounded_but_counts_remain_truthful() {
        assert!(install_for_process());
        let ((), report) = observe(
            async {
                for _ in 0..13 {
                    tracing::event!(
                        target: "sqlx::query",
                        tracing::Level::DEBUG,
                        elapsed_secs = 0.001_f64
                    );
                }
            },
            Instant::now(),
        )
        .await;
        let report = report.unwrap();
        assert_eq!(report.queries, 13);
        assert_eq!(report.captured, MAX_SQL_SAMPLES as u8);
        assert_eq!(report.uncaptured, 5);
        assert_eq!(report.elapsed_us, 13_000);
    }
}
