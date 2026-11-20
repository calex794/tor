use super::QueueProbe;
use std::time::Duration;
use tokio::{sync::oneshot, task::JoinHandle};

pub struct ObservationCollector {
    pub(super) join: JoinHandle<()>,
    pub(super) stop: oneshot::Sender<oneshot::Sender<bool>>,
    pub(super) probe: QueueProbe,
}
impl ObservationCollector {
    /// Forced termination never retires an epoch or certifies its tail.
    pub fn abort(self) {
        self.probe.gate.disable();
        self.join.abort();
    }
    pub fn probe(&self) -> QueueProbe {
        self.probe.clone()
    }
    /// Invoke only after the real HTTP server stopped accepting and drained all
    /// requests. A true result means a *durable retirement acknowledgement*,
    /// not that every queued observation reached PostgreSQL. Inspect the
    /// snapshot's persisted/failed/dropped counters and durable hourly health
    /// before claiming complete measurement. Drop/abort is not this witness.
    pub async fn shutdown_after_http_drained(mut self) -> bool {
        let (ack, done) = oneshot::channel();
        if self.stop.send(ack).is_err() {
            return false;
        }
        let wait = async {
            let clean = done.await.unwrap_or(false);
            clean && (&mut self.join).await.is_ok()
        };
        match tokio::time::timeout(Duration::from_secs(4), wait).await {
            Ok(true) => true,
            _ => {
                self.join.abort();
                false
            }
        }
    }
}
