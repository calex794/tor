use super::*;

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
    health.retire(vanished,0,0, &HtmlIngressStopped::after_http_stopped_and_drained()).await.unwrap();
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
    health.retire(epoch,1,2, &HtmlIngressStopped::after_http_stopped_and_drained()).await.unwrap();
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

