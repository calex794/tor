use super::*;

// This reproduces the scheduler that used to leave an orphan spawn_blocking
// drain behind a timed-out pump. The FIFO handoff must not need that executor.
#[test]
#[ignore = "requires fresh isolated PostgreSQL with HTML health migration"]
fn html_pump_does_not_lose_or_delay_fifo_barriers_on_a_saturated_blocking_executor() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let f = Fixture::new().await;
        let recorder = TrafficRecorder::start(8, 16);
        let mut pump = PgRollupPump::new(PgRilRollups::new(f.app.clone()), recorder.clone());
        pump.run_once().await;
        let (release, wait) = std::sync::mpsc::channel();
        let (ready, started) = tokio::sync::oneshot::channel();
        let blocked = tokio::task::spawn_blocking(move || {
            let _ = ready.send(());
            let _ = wait.recv_timeout(Duration::from_secs(15));
        });
        started.await.unwrap();
        for _ in 0..2 {
            assert!(recorder.try_record_with_flow(event(), Some(page())));
            let result = tokio::time::timeout(Duration::from_secs(3), pump.run_once()).await;
            if result.is_err() {
                let _ = release.send(());
                blocked.await.unwrap();
                panic!("the pump still depends on a queued blocking drain");
            }
        }
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT coalesce(sum(html_requests),0)::bigint FROM ril_flow_observed_hourly"
        ).fetch_one(&f.app).await.unwrap(), 2);
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM ril_flow_health_epochs WHERE NOT retired AND pending_ms IS NOT NULL"
        ).fetch_one(&f.app).await.unwrap(), 0);
        release.send(()).unwrap();
        blocked.await.unwrap();
        assert!(recorder.try_record_with_flow(event(), Some(page())));
        pump.run_once().await;
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT coalesce(sum(html_requests),0)::bigint FROM ril_flow_observed_hourly"
        ).fetch_one(&f.app).await.unwrap(), 3,
            "releasing the blocked executor must not consume a newer batch");
        pump.shutdown(HtmlIngressStopped::after_http_stopped_and_drained()).await.unwrap();
        f.finish().await;
    });
}
