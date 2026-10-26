//! Optional PG-backed RIL-06 pump. Runs outside HTTP and never blocks responses.
//! Enqueue uses a stable UUID across retries; PG claims survive process death.
use crate::{domain::RollupDelta, repositories::PgRilRollups, services::TrafficRecorder};
use uuid::Uuid;

struct Pending {
    id: String,
    delta: RollupDelta,
}

pub struct PgRollupPump {
    repo: PgRilRollups,
    recorder: TrafficRecorder,
    pending: Option<Pending>,
    ticks: u64,
}

impl PgRollupPump {
    pub fn new(repo: PgRilRollups, recorder: TrafficRecorder) -> Self {
        Self {
            repo,
            recorder,
            pending: None,
            ticks: 0,
        }
    }

    /// One bounded progress unit. Failures affect *telemetry only*, not HTTP.
    /// If staging succeeded but PG's response was lost, the same UUID is
    /// retried rather than counting the in-memory delta twice.
    pub async fn run_once(&mut self) {
        if self.pending.is_none() {
            let source = self.recorder.clone();
            let snapshot = tokio::task::spawn_blocking(move || source.drain_for_persistence())
                .await
                .ok()
                .flatten();
            if let Some(delta) = snapshot.filter(|delta| !delta.is_empty()) {
                self.pending = Some(Pending {
                    id: Uuid::new_v4().to_string(),
                    delta,
                });
            }
        }

        if let Some(pending) = self.pending.as_ref() {
            if self.repo.enqueue(&pending.id, &pending.delta).await.is_ok() {
                self.pending = None;
            }
        }

        // A different process may have staged batches but crashed. Recover them
        // independently of whether this process managed to enqueue a new batch.
        for _ in 0..4 {
            let claim = match self.repo.claim_next().await {
                Ok(Some(claim)) => claim,
                Ok(None) | Err(_) => break,
            };
            match self.repo.materialize(&claim).await {
                Ok(true) | Ok(false) => {}
                Err(_) => {
                    let _ = self.repo.retry(&claim).await;
                    break;
                }
            }
        }
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % 360 == 0 {
            let _ = self.repo.prune_expired().await;
        }
    }

    /// One long-lived task per PG-backed web process, sharing its existing pool.
    pub fn spawn_background(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                self.run_once().await;
            }
        })
    }
}
