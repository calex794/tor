use super::*;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with HTML health migration"]
async fn html_ninety_day_windows_keep_the_oldest_count_and_cannot_certify_pruned_hours() {
    let f = Fixture::new().await;
    let anchor = f.hour().await;
    let oldest = anchor - 180 * 86_400;
    sqlx::query("INSERT INTO ril_flow_observed_hourly(bucket,surface_code,acquisition_hint,html_requests) VALUES(to_timestamp($1::bigint),0,3,7),(to_timestamp($1::bigint-3600),0,3,11)")
        .bind(oldest).execute(&f.app).await.unwrap();
    sqlx::query("INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$2,$2+30000)")
        .bind((oldest - 3600) * 1000).bind(anchor * 1000)
        .execute(&f.app).await.unwrap();

    PgRilRollups::new(f.app.clone()).prune_expired().await.unwrap();
    let buckets: Vec<i64> = sqlx::query_scalar("SELECT extract(epoch FROM bucket)::bigint FROM ril_flow_observed_hourly ORDER BY bucket")
        .fetch_all(&f.app).await.unwrap();
    assert_eq!(buckets, vec![oldest], "the exact two-window lower boundary is retained");
    let (counts, health) = PgFlowAudience::new(f.app.clone())
        .observed_health_report(90, "Asia/Kathmandu").await.unwrap();
    let health = health.unwrap();
    assert_eq!(counts.current.until_epoch, health.until_epoch);
    assert_eq!(counts.previous.saved_total, Some(7));
    assert_eq!(health.current.confirmed_hours, 90 * 24);
    assert_eq!(health.previous.confirmed_hours, 90 * 24);

    let older = PgHtmlHealth::new(f.app.clone()).report(90, anchor - 3600).await.unwrap();
    assert_eq!(older.previous.confirmed_hours, 90 * 24 - 1,
        "a durable interval cannot certify a bucket outside retained count history");
    f.finish().await;
}
