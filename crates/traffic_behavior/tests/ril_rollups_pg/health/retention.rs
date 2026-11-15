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

    PgRilRollups::new(f.app.clone())
        .prune_expired()
        .await
        .unwrap();
    let buckets: Vec<i64> = sqlx::query_scalar(
        "SELECT extract(epoch FROM bucket)::bigint FROM ril_flow_observed_hourly ORDER BY bucket",
    )
    .fetch_all(&f.app)
    .await
    .unwrap();
    assert_eq!(
        buckets,
        vec![oldest],
        "the exact two-window lower boundary is retained"
    );
    let (counts, health) = PgFlowAudience::new(f.app.clone())
        .observed_health_report(90, "UTC")
        .await
        .unwrap();
    let health = health.unwrap();
    assert_eq!(counts.current.until_epoch, health.until_epoch);
    assert_eq!(counts.previous.saved_total, Some(7));
    assert_eq!(health.current.confirmed_hours, 90 * 24);
    assert_eq!(health.previous.confirmed_hours, 90 * 24);

    let older = PgHtmlHealth::new(f.app.clone())
        .report(90, anchor - 3600)
        .await
        .unwrap();
    assert_eq!(
        older.previous.confirmed_hours,
        90 * 24 - 1,
        "a durable interval cannot certify a bucket outside retained count history"
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with HTML health migration"]
async fn html_counts_and_health_share_two_complete_equal_windows_at_a_nonboundary_time() {
    let f = Fixture::new().await;
    // This fixture's DB zone is Kathmandu; the comparison boundary is UTC.
    for attempt in 0..2 {
        sqlx::query("DELETE FROM ril_flow_observed_hourly")
            .execute(&f.app)
            .await
            .unwrap();
        sqlx::query("DELETE FROM ril_flow_health_epochs")
            .execute(&f.app)
            .await
            .unwrap();
        let (anchor, db_now): (i64, f64) = sqlx::query_as(
            "SELECT extract(epoch FROM date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')::bigint,extract(epoch FROM clock_timestamp())::double precision")
            .fetch_one(&f.app).await.unwrap();
        assert!(
            db_now > anchor as f64,
            "the sampled DB clock is inside its current hour"
        );
        sqlx::query("INSERT INTO ril_flow_observed_hourly(bucket,surface_code,acquisition_hint,html_requests) VALUES(to_timestamp($1::bigint),0,3,1000),(to_timestamp($1::bigint-3600),0,3,13),(to_timestamp($1::bigint-86400),0,3,11),(to_timestamp($1::bigint-172800),0,3,7),(to_timestamp($1::bigint-176400),0,3,999)")
            .bind(anchor).execute(&f.app).await.unwrap();
        sqlx::query("INSERT INTO ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$2,$2+30000)")
            .bind((anchor - 172800) * 1000).bind(anchor * 1000)
            .execute(&f.app).await.unwrap();
        let (counts, health) = PgFlowAudience::new(f.app.clone())
            .observed_health_report(1, "Europe/Moscow")
            .await
            .unwrap();
        if counts.current.until_epoch != anchor {
            assert_eq!(attempt, 0, "the test cannot cross two UTC hours");
            continue;
        }
        let health = health.unwrap();
        assert_eq!(health.until_epoch, anchor);
        assert_eq!(
            (counts.current.from_epoch, counts.current.until_epoch),
            (anchor - 86400, anchor)
        );
        assert_eq!(
            (counts.previous.from_epoch, counts.previous.until_epoch),
            (anchor - 172800, anchor - 86400)
        );
        assert_eq!(
            (counts.current.hours.len(), counts.previous.hours.len()),
            (24, 24)
        );
        assert_eq!(
            (counts.current.saved_total, counts.previous.saved_total),
            (Some(24), Some(7)),
            "the partial current hour and the hour before the previous window are excluded"
        );
        assert_eq!(
            (
                health.current.confirmed_hours,
                health.previous.confirmed_hours
            ),
            (24, 24)
        );
        assert!(counts
            .current
            .hours
            .iter()
            .all(|h| h.epoch < health.until_epoch));
        f.finish().await;
        return;
    }
    panic!("a stable completed-hour anchor was not obtained");
}
