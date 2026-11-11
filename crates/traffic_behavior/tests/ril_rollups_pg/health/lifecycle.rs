use super::*;

#[tokio::test]
#[ignore="requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_real_pump_keeps_failed_materialization_unknown_and_stops_its_producer(){
    let f=Fixture::new().await;
    let recorder=TrafficRecorder::start(8,16);
    let mut pump=PgRollupPump::new(PgRilRollups::new(f.app.clone()),recorder.clone());
    assert!(!recorder.try_record_with_flow(event(),Some(page())),"no collector proof before schema/epoch readiness");
    assert!(recorder.try_record(event()),"technical telemetry remains independent");
    pump.run_once().await;
    let startup_loss:bool=sqlx::query_scalar("SELECT coalesce(bool_or(had_loss),false) FROM ril_flow_health_hourly").fetch_one(&f.app).await.unwrap();
    assert!(startup_loss,"the rejected sample before first readiness must be durably visible");
    let startup_gap:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ril_flow_health_epochs WHERE retired AND confirmed_ms=started_ms AND lease_until_ms>started_ms)").fetch_one(&f.app).await.unwrap();
    assert!(startup_gap,"a clean counter baseline cannot acknowledge the registration gap");
    assert!(recorder.try_record_with_flow(event(),Some(page())));
    let mut blocker=f.app.begin().await.unwrap();
    sqlx::query("LOCK TABLE ril_flow_observed_hourly IN ACCESS EXCLUSIVE MODE").execute(&mut *blocker).await.unwrap();
    let at=Instant::now();
    pump.run_once().await;
    assert!(at.elapsed()<Duration::from_secs(3));
    let failures:i64=sqlx::query_scalar("SELECT coalesce(sum(failed),0)::bigint FROM ril_flow_health_epochs").fetch_one(&f.app).await.unwrap();
    assert!(failures>0,"known failure is durable before the batch is done");
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM ril_flow_health_epochs WHERE pending_ms IS NOT NULL").fetch_one(&f.app).await.unwrap();
    assert_eq!(pending,1);
    blocker.rollback().await.unwrap();
    sqlx::query("UPDATE ril_flush_batches SET run_after=now() WHERE status='pending'").execute(&f.app).await.unwrap();
    pump.run_once().await;
    let stored:i64=sqlx::query_scalar("SELECT coalesce(sum(html_requests),0)::bigint FROM ril_flow_observed_hourly").fetch_one(&f.app).await.unwrap();
    assert_eq!(stored,1,"retry materializes one observation exactly once");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM ril_flow_health_epochs WHERE pending_ms IS NOT NULL").fetch_one(&f.app).await.unwrap(),0);
    pump.shutdown(HtmlIngressStopped::after_http_stopped_and_drained()).await.unwrap();
    assert!(!recorder.try_record_with_flow(event(),Some(page())));
    assert!(recorder.try_record(event()));
    let retired:bool=sqlx::query_scalar("SELECT bool_and(retired) FROM ril_flow_health_epochs").fetch_one(&f.app).await.unwrap();
    assert!(retired,"graceful stop retires without filling its tail");
    f.finish().await;
}



#[tokio::test]
#[ignore="requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_expired_service_transition_keeps_predecessor_open_until_atomic_replacement() {
    let f=Fixture::new().await;
    let health=PgHtmlHealth::new(f.app.clone());
    let recorder=TrafficRecorder::start(8,16);
    let mut pump=PgRollupPump::new(PgRilRollups::new(f.app.clone()),recorder.clone());
    pump.run_once().await;
    let owned:i64=sqlx::query_scalar("SELECT epoch FROM ril_flow_health_epochs WHERE NOT retired ORDER BY epoch LIMIT 1")
        .fetch_one(&f.app).await.unwrap();
    let anchor=f.hour().await;
    let original=(anchor-49*3600)*1000;
    sqlx::query("UPDATE ril_flow_health_epochs SET started_ms=$2,confirmed_ms=$2+10000,lease_until_ms=$2+30000,pending_ms=NULL WHERE epoch=$1")
        .bind(owned).bind(original).execute(&f.app).await.unwrap();
    sqlx::query("INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$2,$2+30000)")
        .bind((anchor-48*3600)*1000).bind(anchor*1000).execute(&f.app).await.unwrap();
    // Expiration is detected, but the next registration has not happened.
    pump.run_once().await;
    assert!(!recorder.try_record_with_flow(event(),Some(page())));
    assert!(!sqlx::query_scalar::<_,bool>("SELECT retired FROM ril_flow_health_epochs WHERE epoch=$1")
        .bind(owned).fetch_one(&f.app).await.unwrap(),
        "no terminal retirement merely because this collector needs replacement");
    assert_eq!(health.report(1,anchor).await.unwrap().current.confirmed_hours,0);
    let mut lock=f.owner.begin().await.unwrap();
    sqlx::query(&format!("LOCK TABLE {}.ril_flow_health_hourly IN ACCESS EXCLUSIVE MODE",f.schema))
        .execute(&mut *lock).await.unwrap();
    // Persisting the old failure/gap is blocked. The complete handoff rolls back.
    pump.run_once().await;
    lock.rollback().await.unwrap();
    assert!(!sqlx::query_scalar::<_,bool>("SELECT retired FROM ril_flow_health_epochs WHERE epoch=$1")
        .bind(owned).fetch_one(&f.app).await.unwrap());
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM ril_flow_health_epochs WHERE NOT retired")
        .fetch_one(&f.app).await.unwrap(),2,"failed handoff creates no successor");
    assert_eq!(health.report(1,anchor).await.unwrap().current.confirmed_hours,0);
    pump.run_once().await;
    let old=sqlx::query("SELECT retired,confirmed_ms,lease_until_ms FROM ril_flow_health_epochs WHERE epoch=$1")
        .bind(owned).fetch_one(&f.app).await.unwrap();
    assert!(old.get::<bool,_>("retired"));
    assert_eq!(old.get::<i64,_>("confirmed_ms"),original+10000);
    let end:i64=old.get("lease_until_ms");
    let successor:bool=sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ril_flow_health_epochs WHERE NOT retired AND epoch<>$1 AND started_ms=$2)")
        .bind(owned).bind(end).fetch_one(&f.app).await.unwrap();
    assert!(successor,"the successor starts at the same DB instant that closes its predecessor");
    assert_eq!(health.report(1,anchor).await.unwrap().current.confirmed_hours,0,
        "atomic replacement does not repair the preceding unknown interval");
    assert!(recorder.try_record_with_flow(event(),Some(page())));
    // This fixture has no HTTP listener and submits no more FLOW observations:
    // ingress has stopped and drained before the explicit terminal witness.
    pump.shutdown(HtmlIngressStopped::after_http_stopped_and_drained()).await.unwrap();
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with HTML health migration"]
async fn html_owned_background_shutdown_consumes_the_witness_and_never_restarts() {
    let f = Fixture::new().await;
    let recorder = TrafficRecorder::start(8, 16);
    let mut pump = PgRollupPump::new(PgRilRollups::new(f.app.clone()), recorder.clone());
    pump.run_once().await;
    let epoch: i64 = sqlx::query_scalar("SELECT epoch FROM ril_flow_health_epochs WHERE NOT retired")
        .fetch_one(&f.app).await.unwrap();
    assert!(recorder.try_record_with_flow(event(), Some(page())));
    // No HTTP listener exists in this fixture. The final producer above has
    // completed, so the caller can supply its explicit stopped/drained witness.
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = pump.spawn_with_shutdown(receiver);
    sender.send(HtmlIngressStopped::after_http_stopped_and_drained()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), handle).await.unwrap().unwrap().unwrap();
    let (retired, pending): (bool, Option<i64>) = sqlx::query_as(
        "SELECT retired,pending_ms FROM ril_flow_health_epochs WHERE epoch=$1")
        .bind(epoch).fetch_one(&f.app).await.unwrap();
    assert!(retired);
    assert_eq!(pending, None);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT coalesce(sum(html_requests),0)::bigint FROM ril_flow_observed_hourly")
        .fetch_one(&f.app).await.unwrap(), 1, "the bounded final cycle materializes the last completed producer");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM ril_flow_health_epochs WHERE NOT retired")
        .fetch_one(&f.app).await.unwrap(), 0);
    assert!(!recorder.try_record_with_flow(event(), Some(page())));
    assert!(recorder.try_record(event()), "technical telemetry is still independent");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with HTML health migration"]
async fn html_missing_shutdown_witness_and_abort_keep_registered_epochs_unresolved() {
    let f = Fixture::new().await;
    for abort in [false, true] {
        let recorder = TrafficRecorder::start(8, 16);
        let mut pump = PgRollupPump::new(PgRilRollups::new(f.app.clone()), recorder.clone());
        pump.run_once().await;
        let epoch: i64 = sqlx::query_scalar("SELECT max(epoch) FROM ril_flow_health_epochs WHERE NOT retired")
            .fetch_one(&f.app).await.unwrap();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let handle = pump.spawn_with_shutdown(receiver);
        if abort {
            handle.abort();
            assert!(handle.await.unwrap_err().is_cancelled());
        } else {
            drop(sender);
            assert_eq!(tokio::time::timeout(Duration::from_secs(8), handle)
                .await.unwrap().unwrap(), Err(HtmlHealthError::Unavailable));
        }
        assert!(!sqlx::query_scalar::<_,bool>("SELECT retired FROM ril_flow_health_epochs WHERE epoch=$1")
            .bind(epoch).fetch_one(&f.app).await.unwrap(),
            "neither missing witness nor task abort proves HTTP stopped");
        assert!(!recorder.try_record_with_flow(event(), Some(page())));
    }
    f.finish().await;
}
