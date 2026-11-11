use super::*;
use std::time::Duration;
use tokio::sync::oneshot;
use tor_recommendations::domain::{
    exposure::ExposureError,
    exposure_health::{HealthCounters, HealthCut},
};

pub struct Worker {
    pub(super) store: PgExposure,
    pub(super) receiver: mpsc::Receiver<IssuedObservation>,
    pub(super) counters: Arc<Counters>,
    pub(super) gate: Arc<Gate>,
    pub(super) health: PgExposureHealth,
    pub(super) constructed: Instant,
    pub(super) epoch: Option<i64>,
    pub(super) registration: Option<String>,
    pub(super) replace: bool,
    pub(super) pending: Option<(HealthCut, i64)>,
}
impl Worker {
    /// Uses the same managed worker path as production bootstrap.
    pub fn spawn_managed(self) -> ObservationCollector {
        let probe = QueueProbe {
            counters: self.counters.clone(),
            gate: self.gate.clone(),
        };
        let (stop, receiver) = oneshot::channel();
        let join = tokio::spawn(self.run_managed(receiver));
        ObservationCollector { join, stop, probe }
    }
    /// Called before exposing an issuer. Repeated attempts reuse the registration
    /// key, including when the previous SQL commit succeeded but timed out.
    pub async fn prepare(&mut self) -> Result<(), ExposureError> {
        if !self.replace {
            return self.advance().await;
        }
        self.gate.disable();
        if self.registration.is_none() {
            self.registration =
                Some(tor_recommendations::services::exposure::worker_registration_key()?);
        }
        let clock = self.health.clock_ms().await?;
        let gap = startup_gap(clock, self.constructed.elapsed());
        let started = Instant::now();
        let lease = self
            .health
            .start_epoch(
                self.registration
                    .as_deref()
                    .ok_or(ExposureError::Unavailable)?,
                self.epoch,
                gap,
                self.health_counters()?,
            )
            .await?;
        self.epoch = Some(lease.epoch);
        self.registration = None;
        self.replace = false;
        self.pending = None;
        if !self.gate.renew(lease, started) {
            self.stage_replacement();
            return Err(ExposureError::Expired);
        }
        Ok(())
    }
    fn stage_replacement(&mut self) {
        self.gate.disable();
        self.replace = true;
        self.registration = None;
        self.pending = None;
        // The old epoch remains unresolved until the atomic successor commit.
    }
    fn health_counters(&self) -> Result<HealthCounters, ExposureError> {
        Ok(HealthCounters {
            dropped: count(self.counters.dropped.load(Ordering::Acquire))?,
            failed: count(self.counters.failed.load(Ordering::Acquire))?,
        })
    }
    async fn advance(&mut self) -> Result<(), ExposureError> {
        let result = self.advance_inner().await;
        if let Err(error) = result {
            self.gate.disable();
            if error == ExposureError::Expired {
                self.stage_replacement();
            }
        }
        result
    }
    async fn advance_inner(&mut self) -> Result<(), ExposureError> {
        let epoch = self.epoch.ok_or(ExposureError::Unavailable)?;
        if self.pending.is_none() {
            let started = Instant::now();
            let cut = self.health.begin_checkpoint(epoch).await?;
            if !self.gate.renew(cut.lease, started) {
                return Err(ExposureError::Expired);
            }
            // Reserve -> release increment -> send makes a preempted producer
            // visible to this acquire target before a checkpoint can pass it.
            self.pending = Some((cut, count(self.counters.queued.load(Ordering::Acquire))?));
        }
        let (cut, target) = self.pending.ok_or(ExposureError::Unavailable)?;
        let started = Instant::now();
        let (lease, complete) = self
            .health
            .finish_checkpoint(
                cut,
                target,
                count(self.counters.settled.load(Ordering::Acquire))?,
                self.health_counters()?,
            )
            .await?;
        if !self.gate.renew(lease, started) {
            return Err(ExposureError::Expired);
        }
        if complete {
            self.pending = None;
        }
        Ok(())
    }
    pub async fn run(self) {
        self.run_inner(None).await;
    }
    pub(super) async fn run_managed(self, stop: oneshot::Receiver<oneshot::Sender<bool>>) {
        self.run_inner(Some(stop)).await;
    }
    async fn run_inner(mut self, mut stop: Option<oneshot::Receiver<oneshot::Sender<bool>>>) {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                witness=async { match stop.as_mut() {
                    Some(rx)=>rx.await,
                    None=>std::future::pending().await,
                }}=>{
                    if let Ok(ack)=witness {
                        let clean=self.shutdown_after_ingress_drained().await;
                        let _=ack.send(clean);
                        break;
                    }
                    stop=None; // A dropped sender is not a shutdown witness.
                }
                _=tick.tick()=>{
                    if self.prepare().await.is_err() {
                        self.counters.failed.fetch_add(1,Ordering::Release);
                    }
                    if !matches!(tokio::time::timeout(Duration::from_millis(950),self.store.prune()).await,Ok(Ok(_))) {
                        self.counters.failed.fetch_add(1,Ordering::Release);
                    }
                    if self.health.prune().await.is_err() {
                        self.counters.failed.fetch_add(1,Ordering::Release);
                    }
                }
                first=self.receiver.recv(),if self.gate.epoch().is_some()=>{
                    let Some(first)=first else { break; };
                    let mut batch=vec![first];
                    while batch.len()<64 {
                        match self.receiver.try_recv() { Ok(next)=>batch.push(next),Err(_)=>break }
                    }
                    self.store_batch(&batch).await;
                }
            }
        }
        self.gate.disable();
    }
    async fn store_batch(&self, batch: &[IssuedObservation]) {
        // A select guard may have been evaluated before a long scheduling pause.
        let result = match self.gate.epoch() {
            Some(epoch) => tokio::time::timeout(
                Duration::from_millis(950),
                self.store.register_in_epoch(batch, epoch),
            )
            .await
            .ok()
            .and_then(Result::ok),
            None => None,
        };
        let persisted = result.unwrap_or(0).max(0) as u64;
        self.counters
            .persisted
            .fetch_add(persisted, Ordering::Release);
        self.counters.failed.fetch_add(
            (batch.len() as u64).saturating_sub(persisted),
            Ordering::Release,
        );
        self.counters
            .settled
            .fetch_add(batch.len() as u64, Ordering::Release);
    }
    async fn shutdown_after_ingress_drained(&mut self) -> bool {
        self.receiver.close();
        // HTTP is drained. No producer can create a new card/callback now.
        let drain = async {
            self.prepare().await?;
            while let Some(first) = self.receiver.recv().await {
                let mut batch = vec![first];
                while batch.len() < 64 {
                    match self.receiver.try_recv() {
                        Ok(next) => batch.push(next),
                        Err(_) => break,
                    }
                }
                self.store_batch(&batch).await;
            }
            self.advance().await?;
            if self.pending.is_some() {
                return Err(ExposureError::Unavailable);
            }
            let epoch = self.epoch.ok_or(ExposureError::Unavailable)?;
            self.gate.disable();
            self.health
                .retire_after_ingress_drained(epoch, self.health_counters()?)
                .await
        };
        // Timeout/abort leaves the durable epoch unresolved; it is never "clean".
        let completed = matches!(
            tokio::time::timeout(Duration::from_secs(3), drain).await,
            Ok(Ok(()))
        );
        self.gate.disable();
        completed
    }
}
fn count(n: u64) -> Result<i64, ExposureError> {
    i64::try_from(n).map_err(|_| ExposureError::Unavailable)
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Abort/panic stops readiness immediately, but cannot certify a DB tail.
        self.gate.disable();
    }
}

fn startup_gap(clock_ms: i64, age: Duration) -> i64 {
    // DB clock is floored to milliseconds. Ceil monotonic age and keep one
    // additional millisecond so a startup loss cannot slip across a UTC hour.
    let age_ms = age.as_nanos().div_ceil(1_000_000).saturating_add(1);
    let age_ms = i64::try_from(age_ms).unwrap_or(i64::MAX);
    clock_ms.saturating_sub(age_ms).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_gap_keeps_submillisecond_losses_in_the_previous_issued_hour() {
        let hour_ms = 3_600_000;
        let lookback_ms = 902_000;
        let sampled_ms = hour_ms + lookback_ms;
        for age in [
            Duration::ZERO,
            Duration::from_nanos(900_000),
            Duration::from_millis(1),
            Duration::from_nanos(1_000_001),
        ] {
            let first_affected = startup_gap(sampled_ms, age) - lookback_ms;
            assert!(first_affected < hour_ms);
            assert_eq!(first_affected.div_euclid(hour_ms), 0);
        }
        assert_eq!(startup_gap(1, Duration::MAX), 0);
    }
}
