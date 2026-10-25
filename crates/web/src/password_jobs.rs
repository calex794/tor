//! A shared, non-queuing bound on memory-intensive Argon2id operations.
use axum::http::StatusCode;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Limit concurrent password hashing and verification per application instance.
/// Distributed attempt limits still live in PostgreSQL.
#[derive(Clone)]
pub(crate) struct PasswordJobs {
    permits: Arc<Semaphore>,
}

impl PasswordJobs {
    pub(crate) fn new(max_running: usize) -> Self {
        assert!(max_running > 0);
        Self {
            permits: Arc::new(Semaphore::new(max_running)),
        }
    }

    pub(crate) async fn run<F, T>(&self, job: F) -> Result<T, StatusCode>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // Never enqueue unbounded expensive jobs behind Tokio's blocking pool.
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            job()
        })
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn refuses_overload_and_releases_capacity_after_work() {
        let jobs = PasswordJobs::new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let busy = jobs.clone();
        let running = tokio::spawn(async move {
            busy.run(move || {
                started_tx.send(()).expect("worker started");
                release_rx
                    .recv_timeout(Duration::from_secs(10))
                    .expect("finish controlled worker");
                17
            })
            .await
        });
        started_rx.await.expect("worker acquired the only permit");
        assert_eq!(jobs.run(|| 11).await, Err(StatusCode::TOO_MANY_REQUESTS));
        release_tx.send(()).expect("release controlled worker");
        assert_eq!(running.await.expect("join worker"), Ok(17));
        assert_eq!(jobs.run(|| 29).await, Ok(29));
    }
}
