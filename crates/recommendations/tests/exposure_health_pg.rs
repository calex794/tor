//! Registered-worker coverage, replay, lease expiry and real transaction fences.
use sqlx::{postgres::{PgConnectOptions, PgPoolOptions}, Row};
use std::str::FromStr;
use tor_contracts::exposure::{CardObservation, ExposureSurface};
use tor_recommendations::{
    domain::{exposure::*, exposure_health::*},
    repositories::exposure::{PgExposure, PgExposureHealth},
    services::exposure::{issue, worker_registration_key},
};
#[path = "support/flow_fixture.rs"]
mod support;
use support::Fixture;

async fn fixture() -> Fixture {
    Fixture::new(&["flow_observation_epochs", "flow_observation_health_hourly",
        "flow_observation_state", "flow_observation_proofs", "flow_observation_hourly"]).await
}
async fn anchor(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT floor((extract(epoch FROM now())-900)/3600)::bigint*3600")
        .fetch_one(&f.pool).await.unwrap()
}
async fn interval(f: &Fixture, start: i64, confirmed: i64, lease: i64) -> i64 {
    sqlx::query_scalar("INSERT INTO flow_observation_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$2,$3) RETURNING epoch")
        .bind(start).bind(confirmed).bind(lease).fetch_one(&f.pool).await.unwrap()
}
// A test-only clock in a disposable schema exercises exact SQL hour boundaries.
// Explicit pg_catalog in this fixture's search_path permits the local override.
async fn clock(f: &Fixture, millis: i64) {
    sqlx::query(&format!("CREATE TABLE IF NOT EXISTS {}.flow_test_clock(ms bigint NOT NULL)", f.schema))
        .execute(&f.admin).await.unwrap();
    sqlx::query(&format!("DELETE FROM {}.flow_test_clock", f.schema)).execute(&f.admin).await.unwrap();
    sqlx::query(&format!("INSERT INTO {}.flow_test_clock VALUES($1)", f.schema))
        .bind(millis).execute(&f.admin).await.unwrap();
    sqlx::query(&format!("GRANT SELECT ON {}.flow_test_clock TO tor_app", f.schema))
        .execute(&f.admin).await.unwrap();
    sqlx::query(&format!(
        "CREATE OR REPLACE FUNCTION {}.clock_timestamp() RETURNS timestamptz LANGUAGE sql VOLATILE AS 'SELECT to_timestamp(ms/1000.0) FROM {}.flow_test_clock'",
        f.schema, f.schema
    )).execute(&f.admin).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires fresh disposable PostgreSQL18"]
async fn unresolved_expired_worker_cannot_be_masked_by_a_healthy_worker_or_pruned() {
    let f = fixture().await;
    let store = PgExposureHealth::new(f.pool.clone());
    let until = anchor(&f).await;
    assert_eq!(store.report(1, until).await.unwrap().current.confirmed_hours, 0);
    interval(&f, (until-172_800)*1000, until*1000+902_000, until*1000+1_800_000).await;
    let complete = store.report(1, until).await.unwrap();
    assert!(complete.current.complete(1) && complete.previous.complete(1));
    let missing = interval(&f, (until-140_000)*1000, (until-10_800)*1000, (until-10_770)*1000).await;
    let report = store.report(1, until).await.unwrap();
    assert!(report.current.confirmed_hours < 24,
        "worker B must not mask A after A's old lease stops overlapping the window");
    let ancient = interval(&f, (until-200*86_400)*1000, (until-199*86_400)*1000, (until-199*86_400+30)*1000).await;
    store.prune().await.unwrap();
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM flow_observation_epochs WHERE epoch=ANY($1)")
        .bind(vec![missing, ancient]).fetch_one(&f.pool).await.unwrap();
    assert_eq!(kept, 2, "age and lease expiry are not a stopped-process witness");
    let options = PgConnectOptions::from_str(&f.app_url).unwrap().options([
        ("search_path", format!("{},pg_catalog,public", f.schema)),
        ("timezone", "Asia/Kathmandu".into()),
    ]);
    let zone = PgPoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
    assert_eq!(PgExposureHealth::new(zone.clone()).report(1, until).await.unwrap().until_epoch, until);
    assert_eq!(store.report(1, until).await.unwrap().current.confirmed_hours, 0);
    zone.close().await;
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires fresh disposable PostgreSQL18"]
async fn startup_losses_retry_and_atomic_handoff_survive_a_stalled_queue() {
    let f = fixture().await;
    let store = PgExposureHealth::new(f.pool.clone());
    let t = 1_800_000_000_000i64;
    clock(&f, t).await;
    let counters = HealthCounters { dropped: 4, failed: 1 };
    let key = worker_registration_key().unwrap();
    let first = store.start_epoch(&key, None, t-3_600_000, counters).await.unwrap();
    let replay = store.start_epoch(&key, None, t-3_600_000, counters).await.unwrap();
    assert_eq!(first.epoch, replay.epoch);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM flow_observation_epochs").fetch_one(&f.pool).await.unwrap();
    assert_eq!(count, 1, "an ambiguous successful commit is retried, not duplicated");
    let startup: bool = sqlx::query_scalar(
        "SELECT bool_and(lost AND failed AND uncertain) FROM flow_observation_health_hourly WHERE epoch=$1 AND bucket_ms=ANY($2)"
    ).bind(first.epoch).bind(vec![t-3_600_000,t]).fetch_one(&f.pool).await.unwrap();
    assert!(startup, "loss before first ready epoch must not disappear into its baseline");
    clock(&f, t+1000).await;
    let cut = store.begin_checkpoint(first.epoch).await.unwrap();
    assert!(!store.finish_checkpoint(cut, 5, 0, counters).await.unwrap().1);
    clock(&f, t+29_000).await;
    assert!(!store.finish_checkpoint(cut, 5, 0, counters).await.unwrap().1);
    let row = sqlx::query("SELECT confirmed_ms,lease_until_ms FROM flow_observation_epochs WHERE epoch=$1")
        .bind(first.epoch).fetch_one(&f.pool).await.unwrap();
    assert_eq!(row.get::<i64,_>("confirmed_ms"), t);
    assert_eq!(row.get::<i64,_>("lease_until_ms"), t+59_000);
    clock(&f, t+60_000).await;
    assert_eq!(store.begin_checkpoint(first.epoch).await, Err(ExposureError::Expired));
    let next_key = worker_registration_key().unwrap();
    assert_eq!(store.start_epoch(&next_key, Some(first.epoch), t, HealthCounters::default()).await, Err(ExposureError::Invalid));
    let retired: bool = sqlx::query_scalar("SELECT retired FROM flow_observation_epochs WHERE epoch=$1")
        .bind(first.epoch).fetch_one(&f.pool).await.unwrap();
    assert!(!retired, "failed handoff must leave the old worker unresolved");
    let next = store.start_epoch(&next_key, Some(first.epoch), t, counters).await.unwrap();
    let pair: (i64,i64) = sqlx::query_as(
        "SELECT a.ended_ms,b.started_ms FROM flow_observation_epochs a,flow_observation_epochs b WHERE a.epoch=$1 AND b.epoch=$2"
    ).bind(first.epoch).bind(next.epoch).fetch_one(&f.pool).await.unwrap();
    assert_eq!(pair, (t+60_000,t+60_000));
    let preserved: i64 = sqlx::query_scalar("SELECT confirmed_ms FROM flow_observation_epochs WHERE epoch=$1")
        .bind(first.epoch).fetch_one(&f.pool).await.unwrap();
    assert_eq!(preserved, t, "retirement never fabricates confirmation of the stalled tail");
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires fresh disposable PostgreSQL18"]
async fn failure_after_cut_at_exact_hour_taints_both_callback_and_issued_hour() {
    let f = fixture().await;
    let store = PgExposureHealth::new(f.pool.clone());
    let t = 1_800_000_000_000i64;
    clock(&f, t-5000).await;
    let lease = store.start_epoch(&worker_registration_key().unwrap(), None, t-5000, HealthCounters::default()).await.unwrap();
    clock(&f, t-1).await;
    let cut = store.begin_checkpoint(lease.epoch).await.unwrap();
    clock(&f, t).await;
    let counters = HealthCounters { dropped: 0, failed: 1 };
    assert!(store.finish_checkpoint(cut, 3, 3, counters).await.unwrap().1);
    assert!(store.finish_checkpoint(cut, 3, 3, counters).await.unwrap().1);
    let buckets: Vec<i64> = sqlx::query_scalar(
        "SELECT bucket_ms FROM flow_observation_health_hourly WHERE epoch=$1 AND failed ORDER BY bucket_ms"
    ).bind(lease.epoch).fetch_all(&f.pool).await.unwrap();
    assert_eq!(buckets, vec![t-3_600_000,t],
        "floor-ms upper boundary is inclusive, and late callback errors reach the issued cohort");
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires fresh disposable PostgreSQL18"]
async fn registration_and_callback_fences_distinguish_worker_expiry_from_proof_expiry() {
    let f = fixture().await;
    sqlx::query("INSERT INTO flow_observation_state(singleton) VALUES(true)").execute(&f.pool).await.unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO public.books(source_identity,target_language,title,visibility,access_level) VALUES($1,'ru','Health fence','public','basic') RETURNING id"
    ).bind(format!("ci:{}",f.schema)).fetch_one(&f.pool).await.unwrap();
    let health = PgExposureHealth::new(f.pool.clone());
    let store = PgExposure::new(f.pool.clone());
    let t = 1_800_000_000_000i64;
    clock(&f,t).await;
    let lease = health.start_epoch(&worker_registration_key().unwrap(),None,t,HealthCounters::default()).await.unwrap();
    let card = CardObservation { book_id:book, surface:ExposureSurface::Home, editorial:None };
    let (proof, issued) = issue(card).unwrap();
    assert_eq!(store.register_in_epoch(&[issued],lease.epoch).await.unwrap(),1);
    let context = store.describe(&proof).await.unwrap();
    let event = ObservationEvent { proof,kind:EventKind::Rendered,visible_percent:0,
        continuous_visible_ms:0,foreground:true };
    assert_eq!(store.record_in_epoch(&event,&context,None,lease.epoch).await.unwrap(),ObservationReceipt::Recorded);
    clock(&f,t+31_000).await;
    assert_eq!(store.record_in_epoch(&event,&context,None,lease.epoch).await,Err(ExposureError::Unavailable));
    assert_eq!(store.register_in_epoch(&[issue(card).unwrap().1],lease.epoch).await,Err(ExposureError::Unavailable));
    let count: i64 = sqlx::query_scalar("SELECT sum(opportunities)::bigint FROM flow_observation_hourly").fetch_one(&f.pool).await.unwrap();
    assert_eq!(count,1);
    let successor = health.start_epoch(&worker_registration_key().unwrap(),Some(lease.epoch),t,HealthCounters::default()).await.unwrap();
    clock(&f,t+901_000).await;
    // Raw compatibility method isolates normal proof TTL from the production epoch fence.
    assert_eq!(store.record(&event,&context,None).await,Err(ExposureError::Expired));
    assert_eq!(store.record_in_epoch(&event,&context,None,successor.epoch).await,Err(ExposureError::Unavailable));
    sqlx::query("DELETE FROM public.books WHERE id=$1").bind(book).execute(&f.pool).await.unwrap();
    f.cleanup().await;
}
