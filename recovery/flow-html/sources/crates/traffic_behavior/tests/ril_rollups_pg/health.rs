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
    assert_eq!(counts.current.until_epoch,proof.until_epoch);
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
