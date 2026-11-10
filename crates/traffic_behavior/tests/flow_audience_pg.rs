//! FLOW-04 report over a copy of the officially migrated aggregate schema.
//! Fixtures stay in a private namespace of the disposable tor_ci database.
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;
use tor_traffic_behavior::{
    repositories::PgFlowAudience,
    services::audience::{FlowHourlyError, FLOW_AUDIENCE_DAYS},
};
use uuid::Uuid;

struct Fixture {
    owner: PgPool,
    app: PgPool,
    schema: String,
}

impl Fixture {
    async fn new() -> Self {
        let owner_url = std::env::var("MIGRATOR_URL").expect("CI schema owner required");
        let app_url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
        assert!(owner_url.ends_with("/tor_ci") && app_url.ends_with("/tor_ci"));
        assert!(owner_url.contains("127.0.0.1:5432") && app_url.contains("127.0.0.1:5432"));
        let owner = PgPool::connect(&owner_url).await.unwrap();
        let identity: (String, String) = sqlx::query_as("SELECT current_database(),current_user")
            .fetch_one(&owner)
            .await
            .unwrap();
        assert_eq!(identity, ("tor_ci".into(), "tor_migrator".into()));
        let schema = format!("flow_hours_{}", Uuid::new_v4().simple());
        for sql in [
            format!("CREATE SCHEMA {schema}"),
            format!(
                "CREATE TABLE {schema}.ril_flow_hourly (LIKE public.ril_flow_hourly INCLUDING ALL)"
            ),
            format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"),
            format!("GRANT SELECT ON {schema}.ril_flow_hourly TO tor_app"),
        ] {
            sqlx::query(&sql).execute(&owner).await.unwrap();
        }
        let namespace = schema.clone();
        let app = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(2))
            .after_connect(move |connection, _| {
                let namespace = namespace.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={namespace},public"))
                        .execute(&mut *connection)
                        .await?;
                    // A fractional-hour zone catches accidental local-hour truncation.
                    sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&app_url)
            .await
            .unwrap();
        let identity: (String, String) = sqlx::query_as("SELECT current_database(),current_user")
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(identity, ("tor_ci".into(), "tor_app".into()));
        Self { owner, app, schema }
    }

    async fn execute(&self, sql: &str) {
        sqlx::query(&sql.replace("{table}", &format!("{}.ril_flow_hourly", self.schema)))
            .execute(&self.owner)
            .await
            .unwrap();
    }

    async fn finish(self) {
        self.app.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.owner)
            .await
            .unwrap();
        self.owner.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL with official migrations"]
async fn flow_hours_full_two_weeks_and_utc_boundaries() {
    let fixture = Fixture::new().await;
    // 336 hours x 5 surfaces x 4 source hints. More than 100 result groups;
    // the fifth surface, current/future hours and pre-window rows must not leak.
    fixture
        .execute(
            "INSERT INTO {table}
           (bucket,surface_code,acquisition_hint,html_requests,
            opted_in_html_requests,observed_opted_in_visit_starts)
         SELECT date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  - n * interval '1 hour', s::smallint, a::smallint,
                (s+1)*(a+1),0,0
         FROM generate_series(1,336) n CROSS JOIN generate_series(0,4) s
         CROSS JOIN generate_series(0,3) a",
        )
        .await;
    fixture
        .execute(
            "INSERT INTO {table}
           (bucket,surface_code,acquisition_hint,html_requests,
            opted_in_html_requests,observed_opted_in_visit_starts)
         SELECT date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  - n * interval '1 hour',0,0,9000000000,0,0
         FROM unnest(ARRAY[-1,0,337,721]) n",
        )
        .await;
    let repo = PgFlowAudience::new(fixture.app.clone());
    for days in FLOW_AUDIENCE_DAYS {
        let report = repo.report(days).await.unwrap();
        let hours = i64::from(days) * 24;
        assert_eq!(report.days, days);
        assert_eq!(report.current.until_epoch % 3600, 0);
        assert!(report.current.until_epoch <= report.measured_at_epoch);
        assert!(report.measured_at_epoch - report.current.until_epoch < 3600);
        assert_eq!(report.current.from_epoch, report.previous.until_epoch);
        assert_eq!(report.current.from_utc, report.previous.until_utc);
        for period in [&report.current, &report.previous] {
            assert_eq!(period.hours.len(), hours as usize);
            assert_eq!(period.until_epoch - period.from_epoch, hours * 3600);
            let bounds: (i64, i64) = sqlx::query_as(
                "SELECT EXTRACT(EPOCH FROM $1::text::timestamptz)::bigint,
                        EXTRACT(EPOCH FROM $2::text::timestamptz)::bigint",
            )
            .bind(&period.from_utc)
            .bind(&period.until_utc)
            .fetch_one(&fixture.app)
            .await
            .unwrap();
            assert_eq!(bounds, (period.from_epoch, period.until_epoch));
            assert_eq!(period.recorded_hours, [hours as u16; 4]);
            assert_eq!(
                period.saved_views,
                [
                    Some(hours * 10),
                    Some(hours * 20),
                    Some(hours * 30),
                    Some(hours * 40)
                ]
            );
            let epochs: Vec<i64> = sqlx::query_scalar(
                "SELECT EXTRACT(EPOCH FROM value::timestamptz)::bigint
                 FROM unnest($1::text[]) WITH ORDINALITY AS t(value,position)
                 ORDER BY position",
            )
            .bind(
                period
                    .hours
                    .iter()
                    .map(|hour| hour.hour_utc.clone())
                    .collect::<Vec<_>>(),
            )
            .fetch_all(&fixture.app)
            .await
            .unwrap();
            for (index, hour) in period.hours.iter().enumerate() {
                assert_eq!(hour.hour_epoch, period.from_epoch + index as i64 * 3600);
                assert_eq!(hour.views, [Some(10), Some(20), Some(30), Some(40)]);
                assert_eq!(epochs[index], hour.hour_epoch);
                assert!(hour.hour_utc.ends_with(":00:00Z"));
            }
        }
    }
    // Reads never change the source counters.
    let before: String = sqlx::query_scalar(&format!(
        "SELECT sum(html_requests)::text FROM {}.ril_flow_hourly",
        fixture.schema
    ))
    .fetch_one(&fixture.owner)
    .await
    .unwrap();
    repo.report(7).await.unwrap();
    let after: String = sqlx::query_scalar(&format!(
        "SELECT sum(html_requests)::text FROM {}.ril_flow_hourly",
        fixture.schema
    ))
    .fetch_one(&fixture.owner)
    .await
    .unwrap();
    assert_eq!(before, after);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL with official migrations"]
async fn flow_hours_missing_zero_overflow_and_failed_read_remain_distinct() {
    let fixture = Fixture::new().await;
    let repo = PgFlowAudience::new(fixture.app.clone());
    for days in FLOW_AUDIENCE_DAYS {
        let report = repo.report(days).await.unwrap();
        for period in [report.current, report.previous] {
            assert_eq!(period.saved_views, [None; 4]);
            assert_eq!(period.recorded_hours, [0; 4]);
            assert!(period.hours.iter().all(|hour| hour.views == [None; 4]));
        }
    }
    fixture
        .execute(
            "INSERT INTO {table}
           (bucket,surface_code,acquisition_hint,html_requests,
            opted_in_html_requests,observed_opted_in_visit_starts)
         SELECT date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  - n * interval '1 hour', s::smallint, a::smallint, v,opted,starts
         FROM (VALUES(1,0,0,0::bigint,0::bigint,0::bigint),
                      (1,1,0,11,5,2),(1,1,3,13,7,1),(168,2,0,17,4,1),
                      (169,3,0,19,3,1),(336,3,1,23,6,2)) AS seed(n,s,a,v,opted,starts)",
        )
        .await;
    let report = repo.report(7).await.unwrap();
    assert_eq!(
        report.current.hours[167].views,
        [Some(0), Some(24), None, None]
    );
    assert_eq!(report.current.hours[0].views, [None, None, Some(17), None]);
    assert_eq!(
        report.previous.hours[167].views,
        [None, None, None, Some(19)]
    );
    assert_eq!(report.previous.hours[0].views, [None, None, None, Some(23)]);
    assert_eq!(
        report.current.saved_views,
        [Some(0), Some(24), Some(17), None]
    );
    assert_eq!(report.current.saved_opted_in_requests, [Some(0), Some(12), Some(4), None]);
    assert_eq!(report.current.observed_visit_starts, [Some(0), Some(3), Some(1), None]);
    assert_eq!(report.current.recorded_hours, [1, 1, 1, 0]);
    assert_eq!(report.previous.saved_views, [None, None, None, Some(42)]);
    assert_eq!(report.previous.saved_opted_in_requests, [None, None, None, Some(9)]);
    assert_eq!(report.previous.observed_visit_starts, [None, None, None, Some(3)]);
    assert_eq!(report.previous.recorded_hours, [0, 0, 0, 2]);
    for days in [i32::MIN, -1, 0, 2, 30, 90, i32::MAX] {
        assert_eq!(repo.report(days).await, Err(FlowHourlyError::Invalid));
    }
    fixture
        .execute("UPDATE {table} SET html_requests=9223372036854775807 WHERE surface_code=1")
        .await;
    assert_eq!(repo.report(7).await, Err(FlowHourlyError::Unavailable));
    fixture.app.close().await;
    assert_eq!(repo.report(1).await, Err(FlowHourlyError::Unavailable));
    fixture.finish().await;
}
