use super::*;
use crate::domain::{LatencyGroup, RouteCode, StatusGroup};
use tor_contracts::flow::{AcquisitionChannel, RouteSurface};

fn sample(recorder: &TrafficRecorder) {
    assert!(recorder.try_record_with_flow(
        BehaviorEvent {
            route: RouteCode::Books,
            status: StatusGroup::Success,
            latency: LatencyGroup::Under10Ms,
            subject: None,
            at_millis: 1,
        },
        Some(FlowPageview {
            surface: RouteSurface::BookDetail,
            acquisition_hint: AcquisitionChannel::DirectOrUnknown,
        }),
    ));
}
fn pause(recorder: &TrafficRecorder) -> mpsc::Sender<()> {
    let (ready, started) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    recorder.sender.try_send(Message::Pause(ready, wait)).unwrap();
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    resume
}

#[tokio::test]
async fn cancelled_wait_retains_the_late_fifo_reply_and_each_observation_once() {
    let recorder = TrafficRecorder::start(8, 16);
    sample(&recorder);
    let resume = pause(&recorder);
    let mut response = recorder.start_persistence_drain(false).unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut response)
        .await.is_err());
    // The barrier was already enqueued, so later traffic stays in a later batch.
    sample(&recorder);
    resume.send(()).unwrap();
    let first = tokio::time::timeout(Duration::from_secs(2), &mut response)
        .await.unwrap().unwrap();
    assert_eq!(first.flow_rows()[0].html_requests, 1);
    let second = recorder.start_persistence_drain(false).unwrap().await.unwrap();
    assert_eq!(second.flow_rows()[0].html_requests, 1);
    assert!(recorder.start_persistence_drain(false).unwrap().await.unwrap().is_empty());
    assert_eq!(recorder.flow_coverage_counts(), (2, 0));
}

#[tokio::test]
async fn abandoned_receiver_restores_its_batch_before_the_next_fifo_message() {
    let recorder = TrafficRecorder::start(8, 16);
    sample(&recorder);
    let resume = pause(&recorder);
    let response = recorder.start_persistence_drain(false).unwrap();
    drop(response);
    sample(&recorder);
    resume.send(()).unwrap();
    let next = recorder.start_persistence_drain(false).unwrap().await.unwrap();
    assert_eq!(next.flow_rows()[0].html_requests, 2);
    assert!(recorder.start_persistence_drain(false).unwrap().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_managed_barrier_cannot_bypass_a_closed_producer_gate() {
    let recorder = TrafficRecorder::start(8, 16);
    recorder.manage_flow_health();
    assert!(recorder.start_persistence_drain(true).is_none());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    assert!(recorder.activate_flow_health(now, Instant::now()));
    sample(&recorder);
    let response = recorder.start_persistence_drain(true).unwrap();
    recorder.close_flow_health();
    assert!(recorder.start_persistence_drain(true).is_none());
    assert_eq!(response.await.unwrap().flow_rows()[0].html_requests, 1);
}
