use super::*;
use tokio::sync::oneshot;

#[tokio::test]
async fn control_is_shared_and_shutdown_drains_current_attempt() {
    let fence = Arc::new(LaunchFence::new());
    let (started, ready) = oneshot::channel();
    let (stopping, observed) = oneshot::channel();
    let (complete, finish) = oneshot::channel();
    let mail = ManagedMail::spawn(fence.clone(), |mut stop| async move {
        started.send(()).unwrap();
        stop.changed().await.unwrap();
        assert!(*stop.borrow());
        stopping.send(()).unwrap();
        // Simulate a current bounded submission finishing after shutdown.
        finish.await.unwrap();
    });
    ready.await.unwrap();
    assert!(Arc::ptr_eq(&fence, &mail.control().fence));
    mail.control().stop().unwrap();
    assert!(!*mail.stop.signal.borrow());
    let stop = mail.stop_handle();
    stop.request_stop();
    observed.await.unwrap();
    let draining = tokio::spawn(mail.drain());
    assert!(!draining.is_finished());
    complete.send(()).unwrap();
    assert!(draining.await.unwrap());
}

#[tokio::test]
async fn dropped_runtime_cancels_worker_without_a_false_acknowledgement() {
    let fence = Arc::new(LaunchFence::new());
    let (started, ready) = oneshot::channel();
    let (finished, result) = oneshot::channel::<()>();
    let mail = ManagedMail::spawn(fence, move |_| async move {
        let _finished = finished;
        started.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    ready.await.unwrap();
    let stop = mail.stop_handle();
    drop(mail);
    assert!(*stop.signal.borrow());
    assert!(result.await.is_err());
}
