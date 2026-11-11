//! Service state for the durable HTML acknowledgement. No SQL or HTTP work here.
use crate::{
    domain::html_health::{HtmlCheckpoint, HtmlHealthError, HtmlIngressStopped},
    repositories::PgHtmlHealth,
    services::TrafficRecorder,
};
use std::time::{Duration, Instant};

pub(super) struct HtmlPump {
    store: PgHtmlHealth,
    epoch: Option<i64>,
    predecessor: Option<i64>,
    point: Option<HtmlCheckpoint>,
    drained: bool,
    batch: Option<String>,
    failures: u64,
    reported: (u64, u64),
    gap_started: Option<Instant>,
}
impl HtmlPump {
    pub(super) fn new(store: PgHtmlHealth, recorder: &TrafficRecorder) -> Self {
        recorder.manage_flow_health();
        Self {
            store,
            epoch: None,
            predecessor: None,
            point: None,
            drained: false,
            batch: None,
            failures: 0,
            reported: (0, 0),
            gap_started: Some(recorder.flow_health_started()),
        }
    }
    pub(super) fn failure(&mut self, recorder: &TrafficRecorder) {
        self.failures = self.failures.saturating_add(1);
        recorder.close_flow_health();
    }
    pub(super) fn waiting(&self) -> bool {
        self.point.is_some() && self.drained
    }
    pub(super) fn point(&self) -> Option<HtmlCheckpoint> {
        self.point
    }
    pub(super) fn drained(&mut self, point: Option<HtmlCheckpoint>, batch: Option<String>) {
        if matches_checkpoint(self.point, point) {
            self.drained = true;
            self.batch = batch;
        }
    }
    pub(super) async fn prepare(
        &mut self,
        recorder: &TrafficRecorder,
    ) -> Result<(), HtmlHealthError> {
        let started = Instant::now();
        let result =
            tokio::time::timeout(Duration::from_millis(950), self.prepare_inner(recorder)).await;
        match result {
            Ok(Ok(clock)) if recorder.activate_flow_health(clock, started) => Ok(()),
            Ok(Err(HtmlHealthError::Expired)) => {
                self.failure(recorder);
                self.stage_replacement(recorder);
                Err(HtmlHealthError::Expired)
            }
            _ => {
                self.failure(recorder);
                Err(HtmlHealthError::Unavailable)
            }
        }
    }
    async fn prepare_inner(&mut self, recorder: &TrafficRecorder) -> Result<i64, HtmlHealthError> {
        if self.epoch.is_none() {
            let clock = self
                .store
                .ready_clock()
                .await?
                .ok_or(HtmlHealthError::Unavailable)?;
            let gap_started = self.gap_started.ok_or(HtmlHealthError::Unavailable)?;
            // The DB sample preceded this elapsed measurement. Subtraction is
            // a conservative lower bound even if SQL or scheduling was delayed.
            let from = gap_lower_ms(clock, gap_started.elapsed());
            let dropped = recorder.flow_coverage_counts().1;
            let next = match self.predecessor {
                Some(epoch) => {
                    self.store
                        .replace_epoch_after_gap(
                            epoch,
                            from,
                            (cap(dropped), cap(self.failures)),
                            (cap(self.reported.0), cap(self.reported.1)),
                        )
                        .await?
                }
                None => {
                    self.store
                        .start_epoch_after_gap(
                            from,
                            cap(dropped),
                            cap(self.failures),
                            cap(self.reported.0),
                            cap(self.reported.1),
                        )
                        .await?
                }
            };
            self.epoch = Some(next.0);
            self.predecessor = None;
            self.reported = (dropped, self.failures);
            self.gap_started = None;
        }
        if let Some(point) = self.point {
            return self.store.renew(point).await;
        }
        let point = self
            .store
            .begin(self.epoch.ok_or(HtmlHealthError::Unavailable)?)
            .await?;
        self.point = Some(point);
        self.drained = false;
        self.batch = None;
        Ok(point.cut_ms)
    }
    pub(super) async fn complete(
        &mut self,
        recorder: &TrafficRecorder,
        failed: bool,
    ) -> Result<(), HtmlHealthError> {
        if failed {
            self.failures = self.failures.saturating_add(1);
        }
        let (_, dropped) = recorder.flow_coverage_counts();
        if let Some(epoch) = self
            .epoch
            .filter(|_| self.reported != (dropped, self.failures))
        {
            tokio::time::timeout(
                Duration::from_millis(950),
                self.store.note(epoch, cap(dropped), cap(self.failures)),
            )
            .await
            .map_err(|_| HtmlHealthError::Unavailable)??;
            self.reported = (dropped, self.failures);
        }
        let Some(point) = self.point.filter(|_| self.drained) else {
            return Ok(());
        };
        match tokio::time::timeout(
            Duration::from_millis(950),
            self.store.finish(
                point,
                self.batch.as_deref(),
                cap(dropped),
                cap(self.failures),
            ),
        )
        .await
        {
            Ok(Ok(true)) => {
                self.point = None;
                self.batch = None;
                self.drained = false;
                Ok(())
            }
            Ok(Ok(false)) => Ok(()),
            Ok(Err(HtmlHealthError::Expired)) => {
                self.failure(recorder);
                self.stage_replacement(recorder);
                Err(HtmlHealthError::Expired)
            }
            _ => {
                self.failure(recorder);
                Err(HtmlHealthError::Unavailable)
            }
        }
    }
    fn stage_replacement(&mut self, recorder: &TrafficRecorder) {
        self.gap_started.get_or_insert_with(Instant::now);
        recorder.close_flow_health();
        if let Some(epoch) = self.epoch.take() {
            self.predecessor = Some(epoch);
        }
        self.point = None;
        self.drained = false;
        self.batch = None;
    }
    async fn retire(
        &mut self,
        recorder: &TrafficRecorder,
        witness: &HtmlIngressStopped,
    ) -> Result<(), HtmlHealthError> {
        self.gap_started.get_or_insert_with(Instant::now);
        recorder.close_flow_health();
        if let Some(epoch) = self.epoch.or(self.predecessor) {
            let (_, dropped) = recorder.flow_coverage_counts();
            tokio::time::timeout(
                Duration::from_millis(950),
                self.store
                    .retire(epoch, cap(dropped), cap(self.failures), witness),
            )
            .await
            .map_err(|_| HtmlHealthError::Unavailable)??;
        }
        self.epoch = None;
        self.predecessor = None;
        self.point = None;
        self.drained = false;
        self.batch = None;
        Ok(())
    }
    /// Caller must first stop accepting HTTP and drain in-flight middleware.
    /// A task abort/Drop without that witness leaves the epoch unresolved.
    pub(super) async fn shutdown(
        &mut self,
        recorder: &TrafficRecorder,
        witness: &HtmlIngressStopped,
    ) -> Result<(), HtmlHealthError> {
        self.retire(recorder, witness).await
    }
    pub(super) async fn prune(&self) -> Result<(), HtmlHealthError> {
        tokio::time::timeout(Duration::from_millis(950), self.store.prune())
            .await
            .map_err(|_| HtmlHealthError::Unavailable)?
    }
}
// A delayed reply still owns a useful batch, but cannot acknowledge a newer
// epoch, generation, or cut that was registered after its FIFO barrier.
fn matches_checkpoint(current: Option<HtmlCheckpoint>, received: Option<HtmlCheckpoint>) -> bool {
    received.is_some() && current == received
}

fn cap(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn gap_lower_ms(sample_ms: i64, elapsed_after_reply: Duration) -> i64 {
    // Round the age up; the DB sample itself was rounded down. This also covers
    // the two timestamp rounding boundaries without assuming a maximum RTT.
    let age = elapsed_after_reply.as_millis().saturating_add(u128::from(
        !elapsed_after_reply.subsec_nanos().is_multiple_of(1_000_000),
    ));
    sample_ms
        .saturating_sub(i64::try_from(age).unwrap_or(i64::MAX))
        .saturating_sub(1)
        .max(1)
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    #[test]
    fn a_late_barrier_must_match_the_entire_checkpoint() {
        let current = HtmlCheckpoint { epoch: 10, generation: 4, cut_ms: 50_000 };
        assert!(!matches_checkpoint(Some(current), None));
        assert!(!matches_checkpoint(None, Some(current)));
        for old in [
            HtmlCheckpoint { epoch: 9, ..current },
            HtmlCheckpoint { generation: 3, ..current },
            HtmlCheckpoint { cut_ms: 49_999, ..current },
        ] {
            assert!(!matches_checkpoint(Some(current), Some(old)));
        }
        assert!(matches_checkpoint(Some(current), Some(current)));
    }

    #[test]
    fn startup_lower_bound_includes_the_reply_delay_and_hour_boundary() {
        let birth_ms = 3_599_999;
        let sample_ms = 3_600_005;
        let age_at_reply = Duration::from_millis(906);
        // 900ms of reply delay makes the interval earlier, never later than birth.
        assert!(gap_lower_ms(sample_ms, age_at_reply) <= birth_ms);
        assert_eq!(gap_lower_ms(sample_ms, age_at_reply), 3_599_098);
        let rounded = gap_lower_ms(3_600_001, Duration::from_micros(1_001));
        assert_eq!(rounded, 3_599_998);
        assert!(rounded < 3_600_000);
        assert_eq!(gap_lower_ms(5, Duration::from_secs(10)), 1);
    }
}
