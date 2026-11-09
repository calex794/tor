//! FLOW source totals against migrated disposable PostgreSQL. Canonical C facts
//! below use the published DB contract; HTTP/browser tests exercise C's writer.
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use tor_recommendations::{
    domain::feedback::{Feed, FeedItem, FeedbackError, OrganicSource, Surface, ALGORITHM},
    repositories::{audience::PgDiscoveryAudience, PgRecommendations},
    services::{
        audience::{
            DiscoveryChannel, DiscoveryError, DiscoveryPeriod, DiscoverySource,
            DISCOVERY_PERIOD_DAYS,
        },
        feedback::new_ticket,
    },
};

const DAILY_BOOK: i64 = 9_063_000_001;
const SESSION: [u8; 32] = [63; 32];
const COUNTS: [[i64; 4]; 4] = [
    [2, 3, 5, 7],
    [11, 13, 17, 19],
    [68, 41, 43, 47],
    [0, 0, 23, 29],
];

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn flow_source_missing_measurements_are_distinct_from_measured_zero() {
    let app_url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    let migrator_url = std::env::var("MIGRATOR_URL").expect("CI schema owner required");
    assert!(app_url.ends_with("/tor_ci") && migrator_url.ends_with("/tor_ci"));
    let database = PgPool::connect(&migrator_url).await.unwrap();
    let identity: (String, String) = sqlx::query_as("SELECT current_database(),current_user")
        .fetch_one(&database)
        .await
        .unwrap();
    assert_eq!(identity, ("tor_ci".to_owned(), "tor_migrator".to_owned()));
    // Clone actual migrated schemas into an isolated empty namespace. Other
    // suites may have retained daily facts in public; never erase their data.
    sqlx::query("DROP SCHEMA IF EXISTS flow_source_empty CASCADE")
        .execute(&database)
        .await
        .unwrap();
    sqlx::query("CREATE SCHEMA flow_source_empty")
        .execute(&database)
        .await
        .unwrap();
    for table in [
        "rec_quality_daily",
        "organic_discovery_quality_daily",
        "rec_dirty_buckets",
        "organic_discovery_dirty",
        "rec_worker_health",
    ] {
        sqlx::query(&format!(
            "CREATE TABLE flow_source_empty.{table} (LIKE public.{table} INCLUDING ALL)"
        ))
        .execute(&database)
        .await
        .unwrap();
    }
    sqlx::query("GRANT USAGE ON SCHEMA flow_source_empty TO tor_app")
        .execute(&database)
        .await
        .unwrap();
    sqlx::query("GRANT SELECT ON ALL TABLES IN SCHEMA flow_source_empty TO tor_app")
        .execute(&database)
        .await
        .unwrap();
    let isolated = PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path=flow_source_empty,public")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&app_url)
        .await
        .unwrap();
    let store = PgDiscoveryAudience::new(isolated.clone());
    for days in DISCOVERY_PERIOD_DAYS {
        let report = store.report(days).await.unwrap();
        for period in [report.current, report.previous] {
            assert_eq!(period.channels.len(), 4);
            for row in period.channels {
                assert_eq!(metrics(&row), [None; 4]);
            }
        }
        assert!(!report.health.fresh);
        assert_eq!(report.health.last_success_utc, None);
        assert!(!report.health.rec_dirty && !report.health.organic_dirty);
    }
    sqlx::query(
        "INSERT INTO flow_source_empty.organic_discovery_quality_daily
           (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         VALUES((now() AT TIME ZONE 'UTC')::date,1,'catalog',1,0,0,0,0)",
    )
    .execute(&database)
    .await
    .unwrap();
    let measured = store.report(1).await.unwrap();
    assert_eq!(metrics(&measured.current.channels[0]), [Some(0); 4]);
    assert_eq!(metrics(&measured.current.channels[1]), [None; 4]);
    assert_eq!(metrics(&measured.previous.channels[0]), [None; 4]);
    isolated.close().await;
    assert_eq!(store.report(1).await, Err(DiscoveryError::Unavailable));
    assert_eq!(store.report(2).await, Err(DiscoveryError::Invalid));
    sqlx::query("DROP SCHEMA flow_source_empty CASCADE")
        .execute(&database)
        .await
        .unwrap();
}

async fn pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    assert!(url.ends_with("/tor_ci"), "never use production");
    PgPool::connect(&url).await.unwrap()
}

async fn clear_daily_fixture(pool: &PgPool) {
    for table in [
        "rec_quality_daily",
        "organic_discovery_quality_daily",
        "rec_dirty_buckets",
        "organic_discovery_dirty",
    ] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE book_id BETWEEN $1 AND $1+2"
        ))
        .bind(DAILY_BOOK)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn opens_complete(pool: &PgPool, period: &DiscoveryPeriod) -> bool {
    sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM rec_quality_daily
         WHERE bucket_date>=$1::date AND bucket_date<$2::date AND book_opens IS NULL)",
    )
    .bind(&period.from_day)
    .bind(&period.until_day_exclusive)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn metrics(row: &DiscoveryChannel) -> [Option<i64>; 4] {
    [row.clicks, row.book_opens, row.starts, row.completions]
}

fn assert_delta(
    before: &DiscoveryPeriod,
    after: &DiscoveryPeriod,
    expected: [[i64; 4]; 4],
    multiplier: i64,
    rec_opens_complete: bool,
) {
    assert_eq!(after.channels.len(), 4);
    for (i, source) in [
        DiscoverySource::Catalog,
        DiscoverySource::Search,
        DiscoverySource::Recommendations,
        DiscoverySource::Unattributed,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(after.channels[i].source, source);
        for (metric, (old, new)) in metrics(&before.channels[i])
            .into_iter()
            .zip(metrics(&after.channels[i]))
            .enumerate()
        {
            if (i == 3 && metric < 2) || (i == 2 && metric == 1 && !rec_opens_complete) {
                assert_eq!(new, None);
            } else {
                assert_eq!(
                    new,
                    Some(old.unwrap_or(0) + expected[i][metric] * multiplier),
                    "channel {source:?}, metric {metric}"
                );
            }
        }
    }
}

async fn mutation_fingerprint(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT jsonb_build_array(
            (SELECT count(*) FROM rec_assignments),
            (SELECT count(*) FROM organic_discovery_touches),
            (SELECT count(*) FROM organic_discovery_unattributed_reads),
            (SELECT count(*) FROM rec_subjects),
            (SELECT count(*) FROM event_deliveries),
            (SELECT count(*) FROM rec_quality_daily),
            (SELECT count(*) FROM organic_discovery_quality_daily),
            (SELECT count(*) FROM rec_dirty_buckets),
            (SELECT count(*) FROM organic_discovery_dirty),
            (SELECT last_success::text FROM rec_worker_health WHERE singleton))::text",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn flow_source_full_totals_cover_180_days_without_a_row_limit() {
    let pool = pool().await;
    clear_daily_fixture(&pool).await;
    let report = PgDiscoveryAudience::new(pool.clone());
    let mut before = Vec::new();
    for days in DISCOVERY_PERIOD_DAYS {
        let value = report.report(days).await.unwrap();
        let current_complete = opens_complete(&pool, &value.current).await;
        let previous_complete = opens_complete(&pool, &value.previous).await;
        before.push((value, current_complete, previous_complete));
    }

    // Two books for each of 180 days: 180 rows per source even in ONE 90-day
    // period. IDs need not still be public/present; retained facts are anonymous.
    sqlx::query(
        "INSERT INTO rec_quality_daily
           (bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,
            impressions,paired_clicks,unpaired_clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date-d,$1::bigint+b,
                'home','content','similar_by_content',1,'fallback','flow-source-test',
                31,31,37,41,43,47
         FROM generate_series(0,179) d CROSS JOIN generate_series(0,1) b",
    )
    .bind(DAILY_BOOK)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO organic_discovery_quality_daily
           (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date-d,$1::bigint+b,
                c.source,50,c.clicks,c.opens,c.starts,c.completions
         FROM generate_series(0,179) d CROSS JOIN generate_series(0,1) b
         CROSS JOIN (VALUES ('catalog',2,3,5,7),('search',11,13,17,19),
                            ('unattributed',0,0,23,29)) c(source,clicks,opens,starts,completions)",
    )
    .bind(DAILY_BOOK)
    .execute(&pool)
    .await
    .unwrap();
    // Tomorrow and the day immediately before the 180-day window must not leak
    // into either interval. Large sentinels make a one-day error observable.
    sqlx::query(
        "INSERT INTO organic_discovery_quality_daily
           (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date-d,$1::bigint+2,'catalog',5000,5000,5000,5000,5000
         FROM (VALUES (-1),(180)) v(d)",
    )
    .bind(DAILY_BOOK)
    .execute(&pool)
    .await
    .unwrap();
    for (old, current_complete, previous_complete) in before {
        let value = report.report(old.days).await.unwrap();
        assert!(value.measured_at_utc.ends_with('Z'));
        assert!(value.current.includes_partial_today);
        assert!(!value.previous.includes_partial_today);
        assert_eq!(value.previous.until_day_exclusive, value.current.from_day);
        for period in [&value.current, &value.previous] {
            let span: i32 = sqlx::query_scalar("SELECT $1::date-$2::date")
                .bind(&period.until_day_exclusive)
                .bind(&period.from_day)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(span, value.days);
        }
        assert_delta(
            &old.current,
            &value.current,
            COUNTS,
            i64::from(value.days) * 2,
            current_complete,
        );
        assert_delta(
            &old.previous,
            &value.previous,
            COUNTS,
            i64::from(value.days) * 2,
            previous_complete,
        );
    }

    // A historical unknown must not silently become a partial numeric SUM.
    sqlx::query(
        "UPDATE rec_quality_daily SET book_opens=NULL
         WHERE book_id=$1 AND bucket_date=(now() AT TIME ZONE 'UTC')::date",
    )
    .bind(DAILY_BOOK)
    .execute(&pool)
    .await
    .unwrap();
    for days in DISCOVERY_PERIOD_DAYS {
        assert_eq!(
            report.report(days).await.unwrap().current.channels[2].book_opens,
            None
        );
    }

    let old_health: Option<String> =
        sqlx::query_scalar("SELECT last_success::text FROM rec_worker_health WHERE singleton")
            .fetch_optional(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO rec_worker_health(singleton,last_success) VALUES(true,now()-interval '1 hour') ON CONFLICT(singleton) DO UPDATE SET last_success=EXCLUDED.last_success")
        .execute(&pool).await.unwrap();
    for table in ["rec_dirty_buckets", "organic_discovery_dirty"] {
        sqlx::query(&format!("INSERT INTO {table}(bucket_date,book_id) VALUES((now() AT TIME ZONE 'UTC')::date,$1) ON CONFLICT DO NOTHING"))
            .bind(DAILY_BOOK).execute(&pool).await.unwrap();
    }
    let fingerprint = mutation_fingerprint(&pool).await;
    let stale = report.report(7).await.unwrap();
    assert!(!stale.health.fresh);
    assert!(stale.health.last_success_utc.is_some());
    assert!(stale.health.rec_dirty && stale.health.organic_dirty);
    let repeated = report.report(7).await.unwrap();
    assert_eq!(stale.current, repeated.current);
    assert_eq!(stale.previous, repeated.previous);
    assert_eq!(stale.health, repeated.health);
    assert_eq!(fingerprint, mutation_fingerprint(&pool).await);
    sqlx::query("UPDATE rec_worker_health SET last_success=now() WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    assert!(report.report(7).await.unwrap().health.fresh);
    sqlx::query("DELETE FROM rec_worker_health WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    let absent = report.report(7).await.unwrap();
    assert!(!absent.health.fresh);
    assert_eq!(absent.health.last_success_utc, None);
    if let Some(last_success) = old_health {
        sqlx::query(
            "INSERT INTO rec_worker_health(singleton,last_success) VALUES(true,$1::timestamptz)",
        )
        .bind(last_success)
        .execute(&pool)
        .await
        .unwrap();
    }
    clear_daily_fixture(&pool).await;
}

async fn clean_journey(pool: &PgPool) {
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM books WHERE source_identity LIKE 'ci:flow-sources:%'")
            .fetch_all(pool)
            .await
            .unwrap();
    sqlx::query("DELETE FROM event_deliveries WHERE event_id IN(SELECT id FROM outbox_events WHERE dedupe_key LIKE 'flow-sources-test:%')")
        .execute(pool).await.unwrap();
    sqlx::query("DELETE FROM outbox_events WHERE dedupe_key LIKE 'flow-sources-test:%'")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE email_normalized='flow-sources@tor-ci.example'")
        .execute(pool)
        .await
        .unwrap();
    for table in [
        "rec_quality_daily",
        "organic_discovery_quality_daily",
        "rec_dirty_buckets",
        "organic_discovery_dirty",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE book_id=ANY($1)"))
            .bind(&ids)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id=ANY($1))")
        .bind(&ids).execute(pool).await.unwrap();
    sqlx::query("DELETE FROM books WHERE id=ANY($1)")
        .bind(&ids)
        .execute(pool)
        .await
        .unwrap();
}

async fn book_fixture(pool: &PgPool, name: &str) -> (i64, i64, i64) {
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','FLOW source book','public','basic') RETURNING id",
    )
    .bind(format!("ci:flow-sources:{name}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Chapter','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Readable text') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO book_recommendation_audiences(book_id,audience) VALUES($1,'general')")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    (book, chapter, version)
}

async fn canonical_fact(
    pool: &PgPool,
    actor: i64,
    book: (i64, i64, i64),
    completed: bool,
) -> String {
    let event: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    let (table, event_type, authority) = if completed {
        (
            "reader_chapter_completions",
            "ChapterCompletedV1",
            "C-verified-free-position",
        )
    } else {
        (
            "reader_chapter_starts",
            "ChapterStartedV1",
            "C-verified-free-start",
        )
    };
    // Mirrors C's fact + matching domain event in one commit, never a naked
    // browser payload. Production C writing is separately exercised over HTTP.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(&format!("INSERT INTO {table}(user_id,book_id,chapter_id,first_version_id,source_event_id) VALUES($1,$2,$3,$4,$5::uuid)"))
        .bind(actor).bind(book.0).bind(book.1).bind(book.2).bind(&event)
        .execute(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO outbox_events
           (id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
         VALUES($1::uuid,$2,1,'chapter',$3,'flow-sources-test:'||$1,
           jsonb_build_object('user_id',$4::bigint,'book_id',$5::bigint,'chapter_id',$3::bigint,
                              'version_id',$6::bigint,'authority',$7::text),
           ARRAY['recommendation-feedback-v1'])",
    )
    .bind(&event).bind(event_type).bind(book.1).bind(actor).bind(book.0).bind(book.2).bind(authority)
    .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    event
}

async fn refresh(store: &PgRecommendations) {
    for _ in 0..1000 {
        let rec = store.refresh_quality().await.unwrap();
        let organic = store.refresh_organic_quality().await.unwrap();
        if rec + organic == 0 {
            return;
        }
    }
    panic!("fixture left an unexpectedly large materialization queue");
}

async fn accept_twice(store: &PgRecommendations, event: &str) {
    let (a, b) = tokio::join!(
        store.accept_read_event(event),
        store.accept_read_event(event)
    );
    assert_ne!(
        a.unwrap(),
        b.unwrap(),
        "one worker commits, the other sees the receipt"
    );
    assert!(!store.accept_read_event(event).await.unwrap());
}

async fn organic_navigation(
    store: &PgRecommendations,
    actor: i64,
    book: i64,
    source: OrganicSource,
) {
    let ticket = store
        .issue_organic_tickets(actor, &SESSION, source, &[book])
        .await
        .unwrap()
        .remove(0)
        .1;
    let (a, b) = tokio::join!(
        store.activate_organic(actor, &SESSION, &ticket),
        store.activate_organic(actor, &SESSION, &ticket)
    );
    a.unwrap();
    b.unwrap();
    for _ in 0..2 {
        store
            .record_organic_book_open(actor, &SESSION, book)
            .await
            .unwrap();
    }
}

async fn recommendation_navigation(store: &PgRecommendations, actor: i64, book: i64) {
    let feed = Feed {
        items: vec![FeedItem {
            book_id: book.to_string(),
            title: "FLOW source book".to_owned(),
            reason: "similar_by_content".to_owned(),
            source: "content".to_owned(),
            algorithm_version: ALGORITHM,
            ticket: None,
            feedback_revision: Some(0),
        }],
        generation_id: None,
        next_cursor: None,
        personalized: true,
    };
    let ticket = store
        .issue_tickets(actor, &SESSION, Surface::Home, feed)
        .await
        .unwrap()
        .items
        .remove(0)
        .ticket
        .unwrap();
    let (a, b) = tokio::join!(
        store.activate(actor, &SESSION, &ticket),
        store.activate(actor, &SESSION, &ticket)
    );
    assert_eq!(a.unwrap(), book);
    assert_eq!(b.unwrap(), book);
    for _ in 0..2 {
        store.record_book_open(actor, &SESSION, book).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn flow_source_real_navigation_receipts_optout_and_historical_backfill() {
    let pool = pool().await;
    clean_journey(&pool).await;
    let actor: i64 = sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES('flow-sources@tor-ci.example','FLOW reader','$argon2id$test') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let store = PgRecommendations::new(pool.clone());
    let report = PgDiscoveryAudience::new(pool.clone());
    assert!(!store.consent(actor).await.unwrap().enabled);
    assert!(matches!(
        store
            .issue_organic_tickets(actor, &SESSION, OrganicSource::Catalog, &[1])
            .await,
        Err(FeedbackError::Forbidden)
    ));
    store
        .set_consent(actor, true, 0, &new_ticket().unwrap())
        .await
        .unwrap();
    let mut books = Vec::new();
    for name in [
        "catalog",
        "search",
        "recommendations",
        "unattributed",
        "hidden",
    ] {
        books.push(book_fixture(&pool, name).await);
    }
    refresh(&store).await;
    let before = report.report(1).await.unwrap();
    let rec_opens_complete = opens_complete(&pool, &before.current).await;
    organic_navigation(&store, actor, books[0].0, OrganicSource::Catalog).await;
    organic_navigation(&store, actor, books[1].0, OrganicSource::Search).await;
    recommendation_navigation(&store, actor, books[2].0).await;
    // A direct book open without a source ticket creates no navigation fact.
    store
        .record_book_open(actor, &SESSION, books[3].0)
        .await
        .unwrap();
    store
        .record_organic_book_open(actor, &SESSION, books[3].0)
        .await
        .unwrap();
    for book in books.iter().copied().take(4) {
        for completed in [false, true] {
            let event = canonical_fact(&pool, actor, book, completed).await;
            accept_twice(&store, &event).await;
        }
    }
    let hidden_ticket = store
        .issue_organic_tickets(actor, &SESSION, OrganicSource::Search, &[books[4].0])
        .await
        .unwrap()
        .remove(0)
        .1;
    let hidden_event = canonical_fact(&pool, actor, books[4], false).await;
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(books[4].0)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .activate_organic(actor, &SESSION, &hidden_ticket)
            .await,
        Err(FeedbackError::NotFound)
    );
    store
        .record_organic_book_open(actor, &SESSION, books[4].0)
        .await
        .unwrap();
    accept_twice(&store, &hidden_event).await;
    refresh(&store).await;
    let after = report.report(1).await.unwrap();
    assert_delta(
        &before.current,
        &after.current,
        [[1; 4]; 4],
        1,
        rec_opens_complete,
    );
    assert_eq!(before.previous, after.previous);
    let actual_opens: i64 =
        sqlx::query_scalar("SELECT book_opens FROM rec_quality_daily WHERE book_id=$1")
            .bind(books[2].0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        actual_opens, 1,
        "replayed successful opens materialize once"
    );
    let fingerprint = mutation_fingerprint(&pool).await;
    report.report(30).await.unwrap();
    assert_eq!(fingerprint, mutation_fingerprint(&pool).await);

    // Hiding a previously authorized book changes no anonymous historical total.
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(books[0].0)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(after.current, report.report(1).await.unwrap().current);
    verify_book_open_backfill(&pool, books[2].0).await;

    store
        .set_consent(actor, false, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    let pending = report.report(1).await.unwrap();
    assert!(pending.health.rec_dirty && pending.health.organic_dirty);
    refresh(&store).await;
    let erased = report.report(1).await.unwrap();
    assert_eq!(before.current, erased.current);
    let canonical: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_chapter_starts WHERE user_id=$1")
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        canonical, 5,
        "F opt-out never deletes authoritative C facts"
    );
    let retained: Option<i64> = sqlx::query_scalar("SELECT book_opens FROM rec_quality_daily WHERE book_id=$1 AND bucket_date=(now() AT TIME ZONE 'UTC')::date-45")
        .bind(books[2].0).fetch_one(&pool).await.unwrap();
    assert_eq!(
        retained, None,
        "anonymous history outlives raw identity retention"
    );
    clean_journey(&pool).await;
}

async fn verify_book_open_backfill(pool: &PgPool, book: i64) {
    sqlx::query(
        "UPDATE rec_quality_daily SET book_opens=NULL,starts=99,completions=77 WHERE book_id=$1",
    )
    .bind(book)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rec_quality_daily
           (bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,
            impressions,paired_clicks,unpaired_clicks,starts,completions)
         VALUES((now() AT TIME ZONE 'UTC')::date-45,$1,'home','content','similar_by_content',
                1,'fallback','flow-source-test',10,4,2,9,8)",
    )
    .bind(book)
    .execute(pool)
    .await
    .unwrap();
    let migration = include_str!("../../../migrations/202611109063_flow_source_book_opens.sql");
    let (_, backfill) = migration
        .split_once("WITH")
        .expect("migration backfill CTE");
    sqlx::raw_sql(&format!("WITH{backfill}"))
        .execute(pool)
        .await
        .unwrap();
    let current = sqlx::query("SELECT book_opens,starts,completions FROM rec_quality_daily WHERE book_id=$1 AND bucket_date=(now() AT TIME ZONE 'UTC')::date")
        .bind(book).fetch_one(pool).await.unwrap();
    assert_eq!(current.get::<Option<i64>, _>("book_opens"), Some(1));
    assert_eq!(current.get::<i64, _>("starts"), 99);
    assert_eq!(current.get::<i64, _>("completions"), 77);
    let old = sqlx::query("SELECT book_opens,starts,completions FROM rec_quality_daily WHERE book_id=$1 AND bucket_date=(now() AT TIME ZONE 'UTC')::date-45")
        .bind(book).fetch_one(pool).await.unwrap();
    assert_eq!(old.get::<Option<i64>, _>("book_opens"), None);
    assert_eq!(old.get::<i64, _>("starts"), 9);
    assert_eq!(old.get::<i64, _>("completions"), 8);
}
