use super::*;

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


