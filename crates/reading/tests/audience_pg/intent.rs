//! Real PG snapshots from the exact migrated product tables, no analytics schema.
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use tor_reading::{
    repositories::audience::PgAudience,
    services::audience::{intent::IntentCounts, AudienceError},
};

struct Fixture {
    owner: PgPool,
    app: PgPool,
    schema: String,
}
impl Fixture {
    async fn new() -> Self {
        let owner_url = std::env::var("MIGRATOR_URL").unwrap();
        let app_url = std::env::var("TEST_DATABASE_URL").unwrap();
        for url in [&owner_url, &app_url] {
            assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
        }
        let owner = PgPool::connect(&owner_url).await.unwrap();
        let schema = format!(
            "flow_intent_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&owner)
            .await
            .unwrap();
        for table in ["reader_library_items", "reader_book_follows"] {
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&owner)
            .await
            .unwrap();
        }
        for statement in [
            format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"),
            format!("REVOKE ALL ON ALL TABLES IN SCHEMA {schema} FROM tor_app"),
            format!("GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO tor_app"),
        ] {
            sqlx::query(&statement).execute(&owner).await.unwrap();
        }
        let namespace = schema.clone();
        let app = PgPoolOptions::new()
            .max_connections(3)
            .after_connect(move |connection, _| {
                let schema = namespace.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={schema},public"))
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET default_transaction_read_only=on")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&app_url)
            .await
            .unwrap();
        Self { owner, app, schema }
    }
    async fn seed(&self, table: &str, rows: &str) {
        assert!(["reader_library_items", "reader_book_follows"].contains(&table));
        sqlx::query(&format!(
            "INSERT INTO {}.{table}(user_id,book_id,created_at) VALUES {rows}",
            self.schema
        ))
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
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn snapshots_deduplicate_accounts_not_links_and_removals_are_not_history() {
    let f = Fixture::new().await;
    let repo = PgAudience::new(f.app.clone());
    for days in [1, 7, 30, 90] {
        let empty = repo.intent_report(days).await.unwrap();
        assert_eq!(empty.totals, IntentCounts::default());
        assert_eq!(empty.current, IntentCounts::default());
    }
    f.seed("reader_library_items", "(1,11,now()-interval '1 hour'),(1,12,now()-interval '2 hours'),(2,13,now()-interval '8 days'),(1,14,now()-interval '15 days'),(3,15,now()-interval '200 days'),(4,16,now()+interval '1 day')").await;
    f.seed("reader_book_follows", "(1,11,now()-interval '1 hour'),(1,12,now()-interval '8 days'),(2,13,now()-interval '20 days')").await;
    for (days, links, accounts, follows, followers) in [
        (1, 2, 1, 1, 1),
        (7, 2, 1, 1, 1),
        (30, 4, 2, 3, 2),
        (90, 4, 2, 3, 2),
    ] {
        let report = repo.intent_report(days).await.unwrap();
        assert_eq!(
            report.totals,
            IntentCounts {
                library_links: 6,
                library_accounts: 4,
                follow_links: 3,
                follow_accounts: 2
            }
        );
        assert_eq!(
            report.current,
            IntentCounts {
                library_links: links,
                library_accounts: accounts,
                follow_links: follows,
                follow_accounts: followers
            }
        );
        assert_eq!(report.previous.library_links, if days == 7 { 1 } else { 0 });
        assert_eq!(report.previous.follow_links, if days == 7 { 1 } else { 0 });
        let at = chrono::DateTime::parse_from_rfc3339(&report.measured_at_utc).unwrap();
        let from = chrono::DateTime::parse_from_rfc3339(&report.current_from_utc).unwrap();
        let prior = chrono::DateTime::parse_from_rfc3339(&report.previous_from_utc).unwrap();
        assert_eq!((at - from).num_seconds(), i64::from(days) * 86_400);
        assert_eq!((from - prior).num_seconds(), i64::from(days) * 86_400);
    }
    sqlx::query(&format!(
        "DELETE FROM {}.reader_library_items WHERE user_id=1 AND book_id=11",
        f.schema
    ))
    .execute(&f.owner)
    .await
    .unwrap();
    sqlx::query(&format!(
        "DELETE FROM {}.reader_book_follows WHERE user_id=1 AND book_id=11",
        f.schema
    ))
    .execute(&f.owner)
    .await
    .unwrap();
    let removed = repo.intent_report(7).await.unwrap();
    assert_eq!(removed.current.library_links, 1);
    assert_eq!(removed.current.library_accounts, 1);
    assert_eq!(removed.current.follow_links, 0);
    // Re-adding a product choice is one current row, not restoration of its history.
    f.seed("reader_library_items", "(1,11,now())").await;
    assert_eq!(
        repo.intent_report(7).await.unwrap().current.library_links,
        2
    );
    for days in [0, 2, 365] {
        assert_eq!(repo.intent_report(days).await, Err(AudienceError::Invalid));
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn boundaries_are_half_open_and_a_locked_source_is_unavailable_until_recovery() {
    let f = Fixture::new().await;
    f.seed("reader_library_items", "(1,1,'2026-11-09T12:00:00Z'),(1,2,'2026-11-10T12:00:00Z'),(1,3,'2026-11-11T11:59:59Z'),(2,4,'2026-11-11T12:00:00Z'),(3,5,'2026-11-09T11:59:59Z')").await;
    let query = include_str!("../../src/repositories/audience/intent.sql").replace(
        "transaction_timestamp()",
        "TIMESTAMPTZ '2026-11-11T12:00:00Z'",
    );
    let row = sqlx::query(&query)
        .bind(1_i32)
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("library_links"), 5);
    assert_eq!(row.get::<i64, _>("current_library_links"), 2);
    assert_eq!(row.get::<i64, _>("previous_library_links"), 1);
    assert_eq!(row.get::<i64, _>("current_library_accounts"), 1);
    let repo = PgAudience::new(f.app.clone());
    for table in ["reader_library_items", "reader_book_follows"] {
        let mut lock = f.owner.begin().await.unwrap();
        sqlx::query(&format!(
            "LOCK TABLE {}.{table} IN ACCESS EXCLUSIVE MODE",
            f.schema
        ))
        .execute(&mut *lock)
        .await
        .unwrap();
        let started = std::time::Instant::now();
        assert_eq!(repo.intent_report(7).await, Err(AudienceError::Unavailable));
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        lock.rollback().await.unwrap();
        assert_eq!(repo.intent_report(7).await.unwrap().totals.library_links, 5);
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn one_hundred_thousand_links_are_read_only_bounded_and_exact() {
    let f = Fixture::new().await;
    for table in ["reader_library_items", "reader_book_follows"] {
        sqlx::query(&format!("INSERT INTO {}.{table}(user_id,book_id,created_at) SELECT n/10+1,n+1,now()-interval '1 hour' FROM generate_series(0,99999)n", f.schema))
            .execute(&f.owner).await.unwrap();
    }
    let report = PgAudience::new(f.app.clone())
        .intent_report(90)
        .await
        .unwrap();
    assert_eq!(report.totals.library_links, 100_000);
    assert_eq!(report.totals.library_accounts, 10_000);
    assert_eq!(report.totals.follow_links, 100_000);
    assert_eq!(report.current, report.totals);
    assert_eq!(report.previous, IntentCounts::default());
    f.finish().await;
}
