//! Real RIL materialization is the acknowledgement; a heartbeat alone is not.
use super::observed::Fixture;
use sqlx::Row;
use std::time::{Duration,Instant};
use tor_contracts::flow::{AcquisitionChannel,RouteSurface};
use tor_traffic_behavior::{
    domain::{html_health::{HtmlHealthError,HTML_CLOCK_MARGIN_MS},RollupDelta},
    repositories::{PgFlowAudience,PgHtmlHealth,PgRilRollups},
    services::PgRollupPump,
    BehaviorEvent,FlowPageview,LatencyGroup,RouteCode,StatusGroup,TrafficRecorder,
};
use uuid::Uuid;

fn event()->BehaviorEvent {BehaviorEvent{route:RouteCode::Books,status:StatusGroup::Success,latency:LatencyGroup::Under10Ms,subject:None,at_millis:1}}
fn page()->FlowPageview {FlowPageview{surface:RouteSurface::BookDetail,acquisition_hint:AcquisitionChannel::DirectOrUnknown}}

#[tokio::test]
#[ignore="requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_ack_requires_the_materialized_batch_and_fences_replay_and_expired_leases(){
    let f=Fixture::new().await;
    let health=PgHtmlHealth::new(f.app.clone());
    let ril=PgRilRollups::new(f.app.clone());
    assert!(health.ready().await.unwrap());
    let (epoch,clock)=health.start_epoch().await.unwrap();
    sqlx::query("UPDATE ril_flow_health_epochs SET started_ms=$2-10000,confirmed_ms=$2-10000 WHERE epoch=$1").bind(epoch).bind(clock).execute(&f.app).await.unwrap();
    let point=health.begin(epoch).await.unwrap();
    let mut delta=RollupDelta::default();
    delta.record(event(),None);
    delta.record_observed_flow(page(),Some(f.hour().await));
    let id=Uuid::new_v4().to_string();
    ril.enqueue(&id,&delta).await.unwrap();
    assert!(!health.finish(point,Some(&id),0,0).await.unwrap());
    let confirmed:i64=sqlx::query_scalar("SELECT confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1").bind(epoch).fetch_one(&f.app).await.unwrap();
    assert_eq!(confirmed,clock-10000,"staging is not materialization");
    health.renew(point).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1").bind(epoch).fetch_one(&f.app).await.unwrap(),confirmed);
    // Failure evidence survives a reader restart even before the batch settles.
    health.note(epoch,2,1).await.unwrap();
    let flags:(bool,bool)=sqlx::query_as("SELECT bool_or(had_loss),bool_or(had_error) FROM ril_flow_health_hourly WHERE epoch=$1").bind(epoch).fetch_one(&f.app).await.unwrap();
    assert_eq!(flags,(true,true));
    assert_eq!(health.note(epoch,1,1).await,Err(HtmlHealthError::Invalid));
    let claim=ril.claim_next().await.unwrap().unwrap();
    assert_eq!(claim.id,id);
    assert!(ril.materialize(&claim).await.unwrap());
    let (a,b)=tokio::join!(health.finish(point,Some(&id),2,1),health.finish(point,Some(&id),2,1));
    assert!(a.unwrap() && b.unwrap());
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1").bind(epoch).fetch_one(&f.app).await.unwrap(),point.cut_ms-HTML_CLOCK_MARGIN_MS);
    let old=health.begin(epoch).await.unwrap();
    let newer=health.begin(epoch).await.unwrap();
    assert_eq!(health.finish(old,None,2,1).await,Err(HtmlHealthError::Expired));
    sqlx::query("UPDATE ril_flow_health_epochs SET confirmed_ms=$2-10000,lease_until_ms=$2-1 WHERE epoch=$1").bind(epoch).bind(newer.cut_ms).execute(&f.app).await.unwrap();
    assert_eq!(health.begin(epoch).await.unwrap_err(),HtmlHealthError::Expired);
    assert_eq!(health.renew(newer).await.unwrap_err(),HtmlHealthError::Expired);
    assert_eq!(health.finish(newer,None,2,1).await,Err(HtmlHealthError::Expired));
    health.retire(epoch,2,1).await.unwrap();
    assert_eq!(health.begin(epoch).await.unwrap_err(),HtmlHealthError::Expired);
    let fresh=PgHtmlHealth::new(f.app.clone());
    fresh.start_epoch().await.unwrap();
    let row=sqlx::query("SELECT retired,confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1").bind(epoch).fetch_one(&f.app).await.unwrap();
    assert!(row.get::<bool,_>("retired"));
    assert_eq!(row.get::<i64,_>("confirmed_ms"),newer.cut_ms-10000);
    f.finish().await;
}

#[tokio::test]
#[ignore="requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_history_partial_workers_and_losses_are_not_certified_by_a_clean_restart(){
    let f=Fixture::new().await;
    let health=PgHtmlHealth::new(f.app.clone());
    let anchor=f.hour().await;
    let empty=health.report(1,anchor).await.unwrap();
    assert_eq!((empty.current.confirmed_hours,empty.previous.confirmed_hours),(0,0));
    let clean:i64=sqlx::query_scalar("INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$2,$2+30000) RETURNING epoch").bind((anchor-48*3600)*1000).bind(anchor*1000).fetch_one(&f.app).await.unwrap();
    let full=health.report(1,anchor).await.unwrap();
    assert_eq!((full.current.confirmed_hours,full.previous.confirmed_hours),(24,24));
    let crashed:i64=sqlx::query_scalar("INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$1+10000,$2) RETURNING epoch").bind((anchor-3600)*1000).bind(anchor*1000).fetch_one(&f.app).await.unwrap();
    health.start_epoch().await.unwrap();
    assert_eq!(health.report(1,anchor).await.unwrap().current.confirmed_hours,23,"another healthy worker cannot mask the crashed worker tail");
    sqlx::query("UPDATE ril_flow_health_epochs SET confirmed_ms=$2 WHERE epoch=$1").bind(crashed).bind(anchor*1000).execute(&f.app).await.unwrap();
    assert_eq!(health.report(1,anchor).await.unwrap().current.confirmed_hours,24);
    sqlx::query("INSERT INTO ril_flow_health_hourly(epoch,bucket_ms,had_loss,had_error) VALUES($1,$2,true,false)").bind(clean).bind((anchor-25*3600)*1000).execute(&f.app).await.unwrap();
    let report=health.report(1,anchor).await.unwrap();
    assert_eq!(report.current.loss_hours,0);
    assert_eq!(report.previous.loss_hours,1,"previous-window loss survives a new process");
    sqlx::query("UPDATE ril_flow_health_epochs SET confirmed_ms=$2-1 WHERE epoch=$1").bind(crashed).bind(anchor*1000).execute(&f.app).await.unwrap();
    assert_eq!(health.report(1,anchor).await.unwrap().current.confirmed_hours,23,"one unacknowledged millisecond is not a complete hour");
    // Fixture runs in Kathmandu; both sources still use the identical UTC anchor.
    let (counts,proof)=PgFlowAudience::new(f.app.clone()).observed_health_report(1,"UTC").await.unwrap();
    assert_eq!(counts.current.until_epoch,proof.unwrap().until_epoch);
    assert_eq!(counts.current.saved_total,None);
    f.finish().await;
}

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
    pump.shutdown().await;
    assert!(!recorder.try_record_with_flow(event(),Some(page())));
    assert!(recorder.try_record(event()));
    let retired:bool=sqlx::query_scalar("SELECT bool_and(retired) FROM ril_flow_health_epochs").fetch_one(&f.app).await.unwrap();
    assert!(retired,"graceful stop retires without filling its tail");
    drop(pump);
    f.finish().await;
}


#[tokio::test]
#[ignore="requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_expired_unresolved_worker_still_quarantines_later_hours_and_survives_prune() {
    let f=Fixture::new().await;
    let health=PgHtmlHealth::new(f.app.clone());
    let anchor=f.hour().await;
    sqlx::query("INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$2,$2+30000)")
        .bind((anchor-48*3600)*1000).bind(anchor*1000).execute(&f.app).await.unwrap();
    assert_eq!(health.report(1,anchor).await.unwrap().current.confirmed_hours,24);
    let vanished:i64=sqlx::query_scalar(
        "INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$1+10000,$1+30000) RETURNING epoch")
        .bind((anchor-49*3600)*1000).fetch_one(&f.app).await.unwrap();
    let report=health.report(1,anchor).await.unwrap();
    assert_eq!((report.current.confirmed_hours,report.previous.confirmed_hours),(0,0),
        "B's full ranges cannot certify hours long after A's lease ended");
    let old=anchor*1000-183*86_400_000;
    let unresolved:i64=sqlx::query_scalar(
        "INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$1+10000,$1+30000) RETURNING epoch")
        .bind(old).fetch_one(&f.app).await.unwrap();
    let resolved:i64=sqlx::query_scalar(
        "INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms,retired) VALUES($1,$1+10000,$1+30000,true) RETURNING epoch")
        .bind(old).fetch_one(&f.app).await.unwrap();
    health.prune().await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM ril_flow_health_epochs WHERE epoch=$1 AND NOT retired")
        .bind(unresolved).fetch_one(&f.app).await.unwrap(),1,
        "retention is not a lifecycle witness");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM ril_flow_health_epochs WHERE epoch=$1")
        .bind(resolved).fetch_one(&f.app).await.unwrap(),0,
        "ordinary closed history remains bounded");
    health.retire(vanished,0,0).await.unwrap();
    let row=sqlx::query("SELECT confirmed_ms,lease_until_ms,retired FROM ril_flow_health_epochs WHERE epoch=$1")
        .bind(vanished).fetch_one(&f.app).await.unwrap();
    assert!(row.get::<bool,_>("retired"));
    assert_eq!(row.get::<i64,_>("confirmed_ms"),(anchor-49*3600)*1000+10000);
    assert!(row.get::<i64,_>("lease_until_ms")>=anchor*1000,
        "a returning worker closes at its actual return, never at the old lease expiry");
    health.start_epoch().await.unwrap();
    assert_eq!(health.report(1,anchor).await.unwrap().current.confirmed_hours,0);
    f.finish().await;
}

#[tokio::test]
#[ignore="requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_startup_and_registration_gap_losses_survive_a_new_counter_baseline() {
    let f=Fixture::new().await;
    let health=PgHtmlHealth::new(f.app.clone());
    let anchor=f.hour().await;
    sqlx::query("INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$2,$2+30000)")
        .bind((anchor-48*3600)*1000).bind(anchor*1000).execute(&f.app).await.unwrap();
    let from=(anchor-3600)*1000-1;
    let (epoch,registered)=health.start_epoch_after_gap(from,1,2,0,0).await.unwrap();
    let gap=sqlx::query("SELECT epoch,confirmed_ms,lease_until_ms,dropped,failed FROM ril_flow_health_epochs WHERE retired AND started_ms=$1")
        .bind(from).fetch_one(&f.app).await.unwrap();
    let gap_id:i64=gap.get("epoch");
    assert_eq!(gap.get::<i64,_>("confirmed_ms"),from);
    assert_eq!(gap.get::<i64,_>("lease_until_ms"),registered);
    assert_eq!((gap.get::<i64,_>("dropped"),gap.get::<i64,_>("failed")),(1,2));
    for hour in [anchor-7200,anchor-3600] {
        let flags:(bool,bool)=sqlx::query_as("SELECT had_loss,had_error FROM ril_flow_health_hourly WHERE epoch=$1 AND bucket_ms=$2")
            .bind(gap_id).bind(hour*1000).fetch_one(&f.app).await.unwrap();
        assert_eq!(flags,(true,true),"the gap crossed this UTC hour boundary");
    }
    let after_restart=PgHtmlHealth::new(f.app.clone()).report(1,anchor).await.unwrap();
    assert_eq!(after_restart.current.confirmed_hours,22,
        "an empty unacknowledged startup interval is not repaired by a healthy B");
    assert!(after_restart.current.loss_hours>=2 && after_restart.current.error_hours>=2);
    health.retire(epoch,1,2).await.unwrap();
    // A new rejected sample after retirement is not swallowed by the next
    // epoch's initial counter values.
    let (next,_)=health.start_epoch_after_gap(registered,2,2,1,2).await.unwrap();
    assert_ne!(next,epoch);
    let later_loss:bool=sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ril_flow_health_hourly h JOIN ril_flow_health_epochs e USING(epoch) WHERE e.retired AND e.started_ms=$1 AND e.dropped=2 AND h.had_loss)")
        .bind(registered).fetch_one(&f.app).await.unwrap();
    assert!(later_loss);
    assert_eq!(health.start_epoch_after_gap(registered,1,2,2,2).await.unwrap_err(),HtmlHealthError::Invalid);
    f.finish().await;
}

#[tokio::test]
#[ignore="requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_legacy_health_absence_preserves_counts_but_broken_or_locked_health_is_an_error() {
    let f=Fixture::new().await;
    let namespace=format!("{}_legacy",f.schema);
    sqlx::query(&format!("CREATE SCHEMA {namespace}")).execute(&f.owner).await.unwrap();
    for table in ["ril_flow_observed_hourly","ril_flow_observed_receipts"] {
        sqlx::query(&format!("CREATE TABLE {namespace}.{table} (LIKE public.{table} INCLUDING ALL)"))
            .execute(&f.owner).await.unwrap();
    }
    sqlx::query(&format!("CREATE TABLE {namespace}._sqlx_migrations(version bigint PRIMARY KEY,success boolean NOT NULL)"))
        .execute(&f.owner).await.unwrap();
    for statement in [
        format!("GRANT USAGE ON SCHEMA {namespace} TO tor_app"),
        format!("GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {namespace} TO tor_app"),
    ] {
        sqlx::query(&statement).execute(&f.owner).await.unwrap();
    }
    let configured=namespace.clone();
    let legacy=sqlx::postgres::PgPoolOptions::new().max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .after_connect(move|connection,_|{
            let schema=configured.clone();
            Box::pin(async move {
                // Do not inherit the real public health tables in this fixture.
                sqlx::query(&format!("SET search_path={schema}")).execute(&mut *connection).await?;
                sqlx::query("SET statement_timeout='1s'").execute(&mut *connection).await?;
                sqlx::query("SET lock_timeout='75ms'").execute(connection).await?;
                Ok(())
            })
        })
        .connect(&std::env::var("TEST_DATABASE_URL").unwrap()).await.unwrap();
    let anchor=f.hour().await;
    sqlx::query("INSERT INTO ril_flow_observed_hourly VALUES(to_timestamp($1::bigint),0,3,6)")
        .bind(anchor-3600).execute(&legacy).await.unwrap();
    let source=PgFlowAudience::new(legacy.clone());
    let (counts,health)=source.observed_health_report(1,"UTC").await.unwrap();
    assert_eq!(counts.current.saved_total,Some(6));
    assert_eq!(health,None,"missing, unapplied capability is explicitly unknown");
    assert!(!PgHtmlHealth::new(legacy.clone()).ready().await.unwrap());
    sqlx::query("DELETE FROM ril_flow_observed_hourly").execute(&legacy).await.unwrap();
    let (empty,health)=source.observed_health_report(1,"UTC").await.unwrap();
    assert_eq!(empty.current.saved_total,None,"absence of rows is not a proved zero");
    assert_eq!(health,None);
    // An applied marker with absent tables is damage, not a legacy capability.
    sqlx::query("INSERT INTO _sqlx_migrations VALUES(202611294685,true)").execute(&legacy).await.unwrap();
    assert!(matches!(source.observed_health_report(1,"UTC").await,
        Err(tor_traffic_behavior::services::audience::FlowHourlyError::Unavailable)));
    sqlx::query("DELETE FROM _sqlx_migrations").execute(&legacy).await.unwrap();
    sqlx::query(&format!("CREATE TABLE {namespace}.ril_flow_health_epochs (LIKE public.ril_flow_health_epochs INCLUDING ALL)"))
        .execute(&f.owner).await.unwrap();
    assert!(matches!(source.observed_health_report(1,"UTC").await,
        Err(tor_traffic_behavior::services::audience::FlowHourlyError::Unavailable)),
        "one health table is not an absent capability");
    sqlx::query(&format!("CREATE TABLE {namespace}.ril_flow_health_hourly (LIKE public.ril_flow_health_hourly INCLUDING ALL)"))
        .execute(&f.owner).await.unwrap();
    sqlx::query(&format!("GRANT SELECT ON {namespace}.ril_flow_health_epochs,{namespace}.ril_flow_health_hourly TO tor_app"))
        .execute(&f.owner).await.unwrap();
    sqlx::query("INSERT INTO _sqlx_migrations VALUES(202611294685,true)").execute(&legacy).await.unwrap();
    let (_,health)=source.observed_health_report(1,"UTC").await.unwrap();
    assert_eq!(health.unwrap().current.confirmed_hours,0);
    let mut lock=f.owner.begin().await.unwrap();
    sqlx::query(&format!("LOCK TABLE {namespace}.ril_flow_health_epochs IN ACCESS EXCLUSIVE MODE"))
        .execute(&mut *lock).await.unwrap();
    let result=tokio::time::timeout(Duration::from_millis(1500),source.observed_health_report(1,"UTC"))
        .await.unwrap();
    assert!(matches!(result,Err(tor_traffic_behavior::services::audience::FlowHourlyError::Unavailable)),
        "an installed ledger read failure must not fall back to legacy Unknown/200");
    lock.rollback().await.unwrap();
    legacy.close().await;
    sqlx::query(&format!("DROP SCHEMA {namespace} CASCADE")).execute(&f.owner).await.unwrap();
    f.finish().await;
}


#[tokio::test]
#[ignore="requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_done_flow_batch_needs_its_receipt_but_a_technical_batch_does_not() {
    let f=Fixture::new().await;
    let health=PgHtmlHealth::new(f.app.clone());
    let ril=PgRilRollups::new(f.app.clone());
    let (epoch,clock)=health.start_epoch().await.unwrap();
    sqlx::query("UPDATE ril_flow_health_epochs SET started_ms=$2-10000,confirmed_ms=$2-10000 WHERE epoch=$1")
        .bind(epoch).bind(clock).execute(&f.app).await.unwrap();
    let point=health.begin(epoch).await.unwrap();
    let mut flow=RollupDelta::default();
    flow.record_observed_flow(page(),Some(f.hour().await));
    let flow_id=Uuid::new_v4().to_string();
    ril.enqueue(&flow_id,&flow).await.unwrap();
    // The isolated LIKE fixture deliberately lacks production triggers. This
    // tests the owner's acknowledgement port against a mixed/invalid old write.
    sqlx::query("UPDATE ril_flush_batches SET status='done' WHERE batch_id=$1")
        .bind(&flow_id).execute(&f.app).await.unwrap();
    assert!(!health.finish(point,Some(&flow_id),0,0).await.unwrap(),
        "done without the FLOW receipt is not evidence of observed-row persistence");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1")
        .bind(epoch).fetch_one(&f.app).await.unwrap(),clock-10000);
    sqlx::query("UPDATE ril_flush_batches SET status='pending' WHERE batch_id=$1")
        .bind(&flow_id).execute(&f.app).await.unwrap();
    let claim=ril.claim_next().await.unwrap().unwrap();
    assert_eq!(claim.id,flow_id);
    assert!(ril.materialize(&claim).await.unwrap());
    assert!(health.finish(point,Some(&flow_id),0,0).await.unwrap());
    let technical_point=health.begin(epoch).await.unwrap();
    let mut technical=RollupDelta::default();
    technical.record(event(),None);
    let technical_id=Uuid::new_v4().to_string();
    ril.enqueue(&technical_id,&technical).await.unwrap();
    let claim=ril.claim_next().await.unwrap().unwrap();
    assert_eq!(claim.id,technical_id);
    assert!(ril.materialize(&claim).await.unwrap());
    let has_receipt:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ril_flow_observed_receipts WHERE batch_id=$1)")
        .bind(&technical_id).fetch_one(&f.app).await.unwrap();
    assert!(!has_receipt,"a purely technical RIL batch legitimately has no FLOW receipt");
    assert!(health.finish(technical_point,Some(&technical_id),0,0).await.unwrap());
    let discard_point=health.begin(epoch).await.unwrap();
    let mut discarded=RollupDelta::default();
    discarded.record_observed_flow(page(),None);
    let discarded_id=Uuid::new_v4().to_string();
    ril.enqueue(&discarded_id,&discarded).await.unwrap();
    sqlx::query("UPDATE ril_flush_batches SET status='done' WHERE batch_id=$1")
        .bind(&discarded_id).execute(&f.app).await.unwrap();
    assert!(!health.finish(discard_point,Some(&discarded_id),0,0).await.unwrap(),
        "discard-only FLOW batches also need the immutable receipt");
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
    pump.shutdown().await;
    f.finish().await;
}
