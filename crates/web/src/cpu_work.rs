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
    async fn cancelled_request_keeps_both_permits_until_work_finishes() {
        let active = Arc::new(Semaphore::new(1));
        let in_flight = Arc::new(Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        let task = tokio::spawn(run_with_limits(active.clone(), in_flight.clone(), move || {
            let _ = started_tx.send(());
            finish_rx.recv().expect("release the blocking job");
            7_u8
        }));

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
        let in_flight_permit =
            tokio::time::timeout(Duration::from_secs(5), in_flight.acquire())
                .await
                .expect("queue permit returned")
                .expect("queue semaphore open");
        drop((active_permit, in_flight_permit));
    }
}
