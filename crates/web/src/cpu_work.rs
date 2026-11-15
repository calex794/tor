//! Bounded CPU executor for author HTTP requests.
//! Permits stay inside the blocking task, so dropping the HTTP future cannot
//! accidentally free capacity while decompression or encryption continues.
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;

const ACTIVE_JOBS: usize = 2;
const IN_FLIGHT_JOBS: usize = 8;

static ACTIVE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CpuWorkError {
    Busy,
    Unavailable,
}

pub(crate) async fn run<T, F>(work: F) -> Result<T, CpuWorkError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let active = Arc::clone(ACTIVE.get_or_init(|| Arc::new(Semaphore::new(ACTIVE_JOBS))));
    let in_flight = Arc::clone(IN_FLIGHT.get_or_init(|| Arc::new(Semaphore::new(IN_FLIGHT_JOBS))));
    run_with_limits(active, in_flight, work).await
}

async fn run_with_limits<T, F>(
    active: Arc<Semaphore>,
    in_flight: Arc<Semaphore>,
    work: F,
) -> Result<T, CpuWorkError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    // Bound both executable jobs and requests waiting with an owned body.
    let queued = in_flight
        .try_acquire_owned()
        .map_err(|_| CpuWorkError::Busy)?;
    let running = active
        .acquire_owned()
        .await
        .map_err(|_| CpuWorkError::Unavailable)?;

    tokio::task::spawn_blocking(move || {
        // These guards MUST be moved into the closure, not held by the future
        // awaiting JoinHandle: aborting a request must not oversubscribe CPU.
        let _guards = (queued, running);
        work()
    })
    .await
    .map_err(|_| CpuWorkError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cpu_work_runs_off_the_async_executor() {
        let event_loop_thread = std::thread::current().id();
        let worker_thread = run_with_limits(
            Arc::new(Semaphore::new(1)),
            Arc::new(Semaphore::new(1)),
            || std::thread::current().id(),
        )
        .await
        .unwrap();
        assert_ne!(event_loop_thread, worker_thread);
    }

    #[tokio::test]
    async fn active_limit_prevents_parallel_cpu_oversubscription() {
        let active = Arc::new(Semaphore::new(1));
        let in_flight = Arc::new(Semaphore::new(2));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        let first = tokio::spawn(run_with_limits(
            active.clone(),
            in_flight.clone(),
            move || {
                let _ = started_tx.send(());
                finish_rx.recv().expect("release first job");
            },
        ));
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .expect("first job started")
            .expect("first job signal");

        let (next_tx, next_rx) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(run_with_limits(
            active.clone(),
            in_flight.clone(),
            move || {
                let _ = next_tx.send(());
            },
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            while in_flight.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second request queued");

        assert_eq!(active.available_permits(), 0);
        assert_eq!(
            run_with_limits(active.clone(), in_flight.clone(), || ()).await,
            Err(CpuWorkError::Busy)
        );
        assert!(
            !second.is_finished(),
            "second CPU task must wait for the active slot"
        );

        finish_tx.send(()).expect("release first job");
        first
            .await
            .expect("first task joined")
            .expect("first task succeeded");
        second
            .await
            .expect("second task joined")
            .expect("second task succeeded");
        next_rx.await.expect("second job started after first");
        assert_eq!(active.available_permits(), 1);
        assert_eq!(in_flight.available_permits(), 2);
    }

    /// A single-core Tokio executor must still schedule timers while a CPU
    /// job runs. This deterministic check needs no PostgreSQL or S3 fixture.
    #[tokio::test(flavor = "current_thread")]
    async fn single_thread_runtime_runs_heartbeat_during_blocking_author_work() {
        let active = Arc::new(Semaphore::new(1));
        let in_flight = Arc::new(Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let job = tokio::spawn(run_with_limits(active, in_flight, move || {
            let _ = started_tx.send(());
            release_rx.recv().expect("release CPU task");
        }));
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .expect("CPU task started")
            .expect("blocking worker signalled");

        // The 20ms timer cannot complete if the job is running directly on
        // Tokio's only async thread. Release the worker even on a failure.
        let heartbeat = tokio::time::timeout(
            Duration::from_millis(250),
            tokio::time::sleep(Duration::from_millis(20)),
        )
        .await
        .is_ok();
        assert!(!job.is_finished(), "CPU task must still be executing");
        release_tx.send(()).expect("release CPU task");
        job.await
            .expect("join CPU task")
            .expect("CPU task succeeded");
        assert!(
            heartbeat,
            "blocking author work starved the Tokio event loop"
        );
    }

    #[tokio::test]
    async fn cancelled_request_keeps_both_permits_until_work_finishes() {
        let active = Arc::new(Semaphore::new(1));
        let in_flight = Arc::new(Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        let task = tokio::spawn(run_with_limits(
            active.clone(),
            in_flight.clone(),
            move || {
                let _ = started_tx.send(());
                finish_rx.recv().expect("release the blocking job");
                7_u8
            },
        ));

        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .expect("blocking job started")
            .expect("start signal");
        task.abort();
        tokio::task::yield_now().await;

        assert_eq!(active.available_permits(), 0);
        assert_eq!(in_flight.available_permits(), 0);
        assert_eq!(
            run_with_limits(active.clone(), in_flight.clone(), || 8_u8).await,
            Err(CpuWorkError::Busy)
        );

        finish_tx.send(()).expect("finish the blocking job");
        let active_permit = tokio::time::timeout(Duration::from_secs(5), active.acquire())
            .await
            .expect("active permit returned")
            .expect("active semaphore open");
        let in_flight_permit = tokio::time::timeout(Duration::from_secs(5), in_flight.acquire())
            .await
            .expect("queue permit returned")
            .expect("queue semaphore open");
        drop((active_permit, in_flight_permit));
    }
}
