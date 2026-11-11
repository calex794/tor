//! FLOW-09: complete genre summaries and one repeatable-read counter/health snapshot.
use sqlx::{postgres::{PgConnectOptions, PgPoolOptions}, PgPool};
use std::{str::FromStr, time::{Duration, SystemTime, UNIX_EPOCH}};
use tor_recommendations::{
    domain::exposure::{ExposureFilter, ObservationClass},
    repositories::exposure::PgExposure,
};

struct Fixture {
    schema: String,
    owner: PgPool,
    pool: PgPool,
}
fn checked_url(name: &str, role: &str) -> String {
    let raw=std::env::var(name).expect("explicit disposable PostgreSQL URL");
    let opts=PgConnectOptions::from_str(&raw).unwrap();
    assert_eq!(opts.get_host(),"127.0.0.1");
    assert_eq!(opts.get_port(),5432);
    assert_eq!(opts.get_database(),Some("tor_ci"));
    assert_eq!(opts.get_username(),role);
    raw
}
impl Fixture {
    async fn new() -> Self {
        let owner=PgPoolOptions::new().max_connections(3)
            .connect(&checked_url("MIGRATOR_URL","tor_migrator")).await.unwrap();
        let schema=format!("flow_genres_{}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
        assert!(sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM public._sqlx_migrations WHERE version=202611294684 AND success)")
            .fetch_one(&owner).await.unwrap());
        sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&owner).await.unwrap();
        for table in ["flow_observation_hourly","flow_observation_epochs","flow_observation_health_hourly"] {
            sqlx::query(&format!("CREATE TABLE {schema}.{table}(LIKE public.{table} INCLUDING ALL)"))
                .execute(&owner).await.unwrap();
        }
        sqlx::query(&format!("CREATE TABLE {schema}.source_books(id bigint PRIMARY KEY,title text NOT NULL,genres text[] NOT NULL,tags text[] NOT NULL)"))
            .execute(&owner).await.unwrap();
        sqlx::query(&format!("CREATE VIEW {schema}.rec_materializable_books AS SELECT * FROM {schema}.source_books"))
            .execute(&owner).await.unwrap();
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO tor_app")).execute(&owner).await.unwrap();
        sqlx::query(&format!("GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO tor_app")).execute(&owner).await.unwrap();
        sqlx::query(&format!("GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA {schema} TO tor_app")).execute(&owner).await.unwrap();
        let search_path=format!("SET search_path={schema},public");
        let pool=PgPoolOptions::new().max_connections(4).after_connect(move |conn,_| {
            let sql=search_path.clone();
            Box::pin(async move { sqlx::query(&sql).execute(conn).await?; Ok(()) })
        }).connect(&checked_url("TEST_DATABASE_URL","tor_app")).await.unwrap();
        Self { schema,owner,pool }
    }
    async fn book(&self,id:i64,genres:Vec<String>) {
        sqlx::query("INSERT INTO source_books(id,title,genres,tags) VALUES($1,'Current public projection',$2,ARRAY[]::text[])")
            .bind(id).bind(genres).execute(&self.pool).await.unwrap();
    }
    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE",self.schema)).execute(&self.owner).await.unwrap();
        self.owner.close().await;
    }
}
fn filter() -> ExposureFilter {
    ExposureFilter { days:1,surface:None,after:0,class:None,group:None,campaign:None,editorial:None }
}

#[tokio::test]
#[ignore="requires disposable PostgreSQL 18 and current migrations"]
async fn full_genre_totals_include_low_and_unknown_groups_beyond_detail_100() {
    let f=Fixture::new().await;
    let mut many=(0..101).map(|n|format!("a{n:03}")).collect::<Vec<_>>();
    many.push("a000".into()); // Same book/label must contribute once.
    f.book(1,many).await;
    f.book(2,vec!["zzz-low <script>".into()]).await;
    f.book(3,vec!["zzzz-unknown".into(),"".into(),"   ".into()]).await;
    for (book,views) in [(1_i64,6_i64),(2,0)] {
        sqlx::query("INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities,rendered,views)
            VALUES(date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC',$1,'home',20,20,$2)")
            .bind(book).bind(views).execute(&f.pool).await.unwrap();
    }
    let store=PgExposure::new(f.pool.clone());
    let mut selection=filter();selection.days=7;
    let report=store.report(&selection).await.unwrap();
    assert_eq!(report.current.opportunities,40);
    assert_eq!(report.current.views,6);
    assert_eq!((report.genres.genres,report.genres.low_observed,report.genres.unknown),(103,1,1));
    assert_eq!(report.genres.low_labels,vec!["zzz-low <script>".to_string()]);
    assert_eq!(report.groups.len(),100);
    assert!(!report.groups.iter().any(|g|g.label=="zzz-low <script>"));
    let duplicated=report.groups.iter().find(|g|g.label=="a000").unwrap();
    assert_eq!((duplicated.books,duplicated.counts.opportunities,duplicated.counts.views),(1,20,6));
    let mut paged=selection;paged.after=500;paged.class=Some(ObservationClass::LowObserved);
    let empty=store.report(&paged).await.unwrap();
    assert!(empty.rows.is_empty());
    assert_eq!(empty.genres,report.genres,"detail pagination/class cannot manufacture a global zero");
    f.cleanup().await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="requires disposable PostgreSQL 18 and current migrations"]
async fn closed_counts_and_health_cannot_come_from_different_commits() {
    let f=Fixture::new().await;
    f.book(1,vec!["genre".into()]).await;
    // This tests one MVCC snapshot, not the proof/grace boundary. Obtain the
    // actual owner's anchor and keep both counts and health far from its edge.
    let until=PgExposure::new(f.pool.clone()).report_closed_with_health(&filter())
        .await.unwrap().0.until_epoch;
    sqlx::query("INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities,rendered,views)
        VALUES(to_timestamp(($1-7200)::double precision),1,'home',20,20,1)")
        .bind(until).execute(&f.pool).await.unwrap();
    let epoch:i64=sqlx::query_scalar("INSERT INTO flow_observation_epochs(started_ms,confirmed_ms,lease_until_ms)
        VALUES(($1-172800)*1000,($1-7200)*1000,($1+7200)*1000) RETURNING epoch")
        .bind(until).fetch_one(&f.pool).await.unwrap();
    let gate=(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() & 0x3fff_ffff_ffff_ffff) as i64;
    let signal=gate+1;
    sqlx::query(&format!("CREATE FUNCTION {}.snapshot_gate() RETURNS boolean LANGUAGE plpgsql VOLATILE AS $gate$
        BEGIN PERFORM pg_advisory_xact_lock({signal});
          WHILE NOT pg_try_advisory_xact_lock({gate}) LOOP PERFORM pg_sleep(0.002); END LOOP;
          RETURN true;
        END; $gate$",f.schema)).execute(&f.owner).await.unwrap();
    sqlx::query(&format!("CREATE OR REPLACE VIEW {}.rec_materializable_books AS
        SELECT * FROM {}.source_books WHERE {}.snapshot_gate()",f.schema,f.schema,f.schema))
        .execute(&f.owner).await.unwrap();
    let mut writer=f.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(gate).execute(&mut *writer).await.unwrap();
    let store=PgExposure::new(f.pool.clone());
    let reading=tokio::spawn(async move { store.report_closed_with_health(&filter()).await });
    // The view announces that the report's snapshot is established before waiting.
    // try-lock polling in the fixture avoids altering the owner's 75ms lock timeout.
    tokio::time::timeout(Duration::from_millis(500),async {
        loop {
            let entered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory'
                AND classid=($1::bigint>>32)::oid AND objid=($1::bigint&4294967295)::oid AND objsubid=1 AND granted)")
                .bind(signal).fetch_one(&f.pool).await.unwrap();
            if entered { break; }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }).await.expect("snapshot gate was not entered within the source budget");
    sqlx::query("UPDATE flow_observation_hourly SET views=9 WHERE book_id=1").execute(&mut *writer).await.unwrap();
    sqlx::query("UPDATE flow_observation_epochs SET confirmed_ms=($2+7200)*1000 WHERE epoch=$1")
        .bind(epoch).bind(until).execute(&mut *writer).await.unwrap();
    writer.commit().await.unwrap();
    let (report,health)=reading.await.unwrap().unwrap();
    assert_eq!(report.current.views,1);
    assert_eq!(health.current.error_hours,0);
    assert!(health.current.confirmed_hours<24,"a later complete checkpoint must not certify older counts");
    assert_eq!(report.until_epoch,health.until_epoch);
    let (next,next_health)=PgExposure::new(f.pool.clone()).report_closed_with_health(&filter()).await.unwrap();
    assert_eq!(next.current.views,9);
    assert_eq!(next_health.current.error_hours,0);
    assert_eq!(next_health.current.confirmed_hours,24);
    f.cleanup().await;
}

