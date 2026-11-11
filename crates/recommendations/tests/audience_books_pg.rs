//! Real migrated PostgreSQL: full ranges, bounded pages, current public metadata,
//! retained unknown values and no mutation while reading the admin report.
use sqlx::{postgres::PgPoolOptions, PgPool};
use tor_recommendations::{
    repositories::audience::PgDiscoveryAudience,
    services::audience::{
        books::{BookDiscoveryMetrics, BookDiscoveryQuery, BookDiscoveryReport},
        DiscoveryError, DISCOVERY_PERIOD_DAYS,
    },
};

struct Fixture {
    owner: PgPool,
    app: PgPool,
    schema: &'static str,
    base: i64,
}

impl Fixture {
    async fn new(schema: &'static str, base: i64, count: i32) -> Self {
        assert!(schema.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'));
        let app_url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
        let owner_url = std::env::var("MIGRATOR_URL").expect("CI schema owner required");
        assert!(app_url.ends_with("/tor_ci") && owner_url.ends_with("/tor_ci"));
        let owner = PgPool::connect(&owner_url).await.unwrap();
        let identity: (String, String) = sqlx::query_as("SELECT current_database(),current_user")
            .fetch_one(&owner)
            .await
            .unwrap();
        assert_eq!(identity, ("tor_ci".into(), "tor_migrator".into()));
        sqlx::query("DELETE FROM books WHERE id BETWEEN $1 AND $1+199")
            .bind(base)
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books(id,source_identity,target_language,title,visibility,access_level,genres,tags)
             SELECT $1::bigint+i,'ci:flow:books:'||($1::bigint+i)::text,'ru','Книга '||i,
                    'public','basic',ARRAY['flow-books-group',CASE WHEN i%2=0 THEN 'even' ELSE 'odd' END],
                    ARRAY[CASE WHEN i%3=0 THEN 'flow-books-tag' ELSE 'other' END]
             FROM generate_series(0,$2::int-1) i",
        ).bind(base).bind(count).execute(&owner).await.unwrap();
        sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&owner)
            .await
            .unwrap();
        for table in [
            "rec_quality_daily",
            "organic_discovery_quality_daily",
            "rec_analytics_invalidations",
            "rec_dirty_buckets",
            "organic_discovery_dirty",
            "rec_worker_health",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&owner)
            .await
            .unwrap();
        }
        // Preserve the actual current eligibility view, only limit fixture IDs.
        sqlx::query(&format!(
            "CREATE VIEW {schema}.rec_public_books AS SELECT * FROM public.rec_public_books WHERE id BETWEEN {base} AND {base}+199"
        )).execute(&owner).await.unwrap();
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(&format!(
            "GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO tor_app"
        ))
        .execute(&owner)
        .await
        .unwrap();
        let search_path = format!("SET search_path={schema},public");
        let app = PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let command = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&command).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&app_url)
            .await
            .unwrap();
        Self {
            owner,
            app,
            schema,
            base,
        }
    }

    async fn finish(self) {
        self.app.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM books WHERE id BETWEEN $1 AND $1+199")
            .bind(self.base)
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

fn query(
    days: i32,
    genre: Option<&str>,
    tag: Option<&str>,
    after: Option<i64>,
) -> BookDiscoveryQuery {
    BookDiscoveryQuery::new(
        days,
        genre.map(str::to_owned),
        tag.map(str::to_owned),
        after,
    )
    .unwrap()
}

fn values(value: &BookDiscoveryMetrics) -> [Option<i64>; 5] {
    [
        value.issued,
        value.impressions,
        value.clicks,
        value.book_opens,
        value.starts,
    ]
}

fn assert_totals(report: &BookDiscoveryReport, days: i32) {
    let n = i64::from(days) * 2;
    assert_eq!(
        values(&report.current.totals),
        [
            Some(42 * n + 2),
            Some(7 * n),
            Some(53 * n),
            Some(61 * n),
            Some(118 * n + 1)
        ]
    );
    assert_eq!(
        values(&report.previous.totals),
        [
            Some(42 * n),
            Some(7 * n),
            Some(53 * n),
            Some(61 * n),
            Some(118 * n)
        ]
    );
    assert_eq!(report.matched_books, 26);
    assert_eq!(report.current.counts.no_observations, 22);
    assert_eq!(report.current.counts.observed_without_starts, 1);
    assert_eq!(report.current.counts.with_starts, 3);
    assert_eq!(report.previous.counts.no_observations, 24);
    assert_eq!(report.previous.counts.observed_without_starts, 0);
    assert_eq!(report.previous.counts.with_starts, 2);
    assert!(report.current.includes_partial_today);
    assert!(!report.previous.includes_partial_today);
    assert_eq!(report.previous.until_day_exclusive, report.current.from_day);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn flow_books_full_periods_filters_paging_and_current_public_eligibility() {
    let fixture = Fixture::new("flow_books_totals", 9_065_000_000, 33).await;
    let owner = &fixture.owner;
    let base = fixture.base;
    for (offset, update) in [
        (26, "visibility='draft'"),
        (27, "admin_hold=true"),
        (28, "is_deleted=true"),
        (29, "visibility_status='hidden'"),
        (30, "access_level='adult'"),
        (31, "lifecycle_status='pending_deletion'"),
    ] {
        sqlx::query(&format!("UPDATE books SET {update},genres=ARRAY['secret-genre'],tags=ARRAY['secret-tag'] WHERE id=$1"))
            .bind(base+offset).execute(owner).await.unwrap();
    }
    sqlx::query("INSERT INTO book_recommendation_audiences(book_id,audience) VALUES($1,'adult')")
        .bind(base + 32)
        .execute(owner)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO flow_books_totals.rec_quality_daily
           (bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,
            impressions,paired_clicks,unpaired_clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date-d,$1::bigint+b,
                'home','content','similar_by_content',1,'fallback','flow-books-test',7,2,3,5,11,0
         FROM generate_series(0,179) d CROSS JOIN generate_series(0,1) b",
    )
    .bind(base)
    .execute(owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_books_totals.organic_discovery_quality_daily
           (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date-d,$1::bigint+b,
                c.source,c.issued,c.clicks,c.opens,c.starts,0
         FROM generate_series(0,179) d CROSS JOIN generate_series(0,1) b
         CROSS JOIN (VALUES ('catalog',13,17,19,23),('search',29,31,37,41),
                            ('unattributed',0,0,0,43)) c(source,issued,clicks,opens,starts)",
    )
    .bind(base)
    .execute(owner)
    .await
    .unwrap();
    // Sentinels cover every current-eligibility exclusion, tomorrow and day 181.
    sqlx::query(
        "INSERT INTO flow_books_totals.organic_discovery_quality_daily
           (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date,$1::bigint+i,'catalog',9999,9999,9999,9999,0
         FROM generate_series(26,32) i
         UNION ALL
         SELECT (now() AT TIME ZONE 'UTC')::date-d,$1,'catalog',9999,9999,9999,9999,0
         FROM (VALUES (-1),(180)) v(d)",
    )
    .bind(base)
    .execute(owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_books_totals.organic_discovery_quality_daily
           (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date,$1::bigint+i,source,issued,0,0,starts,0
         FROM (VALUES (2,'catalog',2,0),(3,'catalog',0,0),(4,'unattributed',0,1)) v(i,source,issued,starts)",
    ).bind(base).execute(owner).await.unwrap();
    let store = PgDiscoveryAudience::new(fixture.app.clone());
    let before: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rec_quality_daily),(SELECT count(*) FROM organic_discovery_quality_daily)"
    ).fetch_one(&fixture.app).await.unwrap();
    for days in DISCOVERY_PERIOD_DAYS {
        let report = store
            .books_report(&query(days, None, None, None))
            .await
            .unwrap();
        assert_totals(&report, days);
        assert_eq!(report.rows.len(), 20);
        assert_eq!(report.next_after_id, Some(base + 19));
        assert_eq!(report.rows[0].book_id, base);
        assert_eq!(report.rows[19].book_id, base + 19);
        assert_eq!(
            values(&report.rows[2].current),
            [Some(2), None, Some(0), Some(0), Some(0)]
        );
        assert_eq!(
            values(&report.rows[3].current),
            [Some(0), None, Some(0), Some(0), Some(0)]
        );
        assert_eq!(
            values(&report.rows[4].current),
            [None, None, None, None, Some(1)]
        );
        assert_eq!(values(&report.rows[5].current), [None; 5]);
        assert!(!report.genres.iter().any(|x| x == "secret-genre"));
        assert!(!report.tags.iter().any(|x| x == "secret-tag"));
        assert!(!report.health.fresh);
        let next = store
            .books_report(&query(days, None, None, report.next_after_id))
            .await
            .unwrap();
        assert_totals(&next, days);
        assert_eq!(
            next.rows.iter().map(|row| row.book_id).collect::<Vec<_>>(),
            (base + 20..base + 26).collect::<Vec<_>>()
        );
        assert_eq!(next.next_after_id, None);
    }
    let after: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rec_quality_daily),(SELECT count(*) FROM organic_discovery_quality_daily)"
    ).fetch_one(&fixture.app).await.unwrap();
    assert_eq!(
        before, after,
        "admin reads must not rebuild or modify facts"
    );
    let filtered = store
        .books_report(&query(90, Some(" EVEN "), Some("flow-books-tag"), None))
        .await
        .unwrap();
    assert_eq!(filtered.matched_books, 5);
    assert_eq!(
        filtered
            .rows
            .iter()
            .map(|row| row.book_id)
            .collect::<Vec<_>>(),
        vec![base, base + 6, base + 12, base + 18, base + 24]
    );
    assert_eq!(
        values(&filtered.current.totals),
        [
            Some(42 * 90),
            Some(7 * 90),
            Some(53 * 90),
            Some(61 * 90),
            Some(118 * 90)
        ]
    );
    assert_eq!(filtered.current.counts.no_observations, 4);
    assert_eq!(filtered.current.counts.with_starts, 1);
    // NULL in any retained REC bucket poisons opens, never the independent fields.
    sqlx::query("UPDATE flow_books_totals.rec_quality_daily SET book_opens=NULL WHERE book_id=$1 AND bucket_date=(now() AT TIME ZONE 'UTC')::date-45")
        .bind(base+1).execute(owner).await.unwrap();
    let incomplete = store
        .books_report(&query(90, None, None, None))
        .await
        .unwrap();
    assert_eq!(incomplete.current.totals.book_opens, None);
    assert_eq!(incomplete.rows[1].current.book_opens, None);
    assert_eq!(incomplete.rows[0].current.book_opens, Some(61 * 90));
    assert_eq!(incomplete.previous.totals.book_opens, Some(61 * 180));
    assert_eq!(
        store
            .books_report(&query(1, None, None, None))
            .await
            .unwrap()
            .current
            .totals
            .book_opens,
        Some(61 * 2)
    );
    let filtered = store
        .books_report(&query(90, Some("even"), Some("flow-books-tag"), None))
        .await
        .unwrap();
    assert_eq!(filtered.current.totals.book_opens, Some(61 * 90));
    // Today's taxonomy determines membership; no duplicated multi-genre totals.
    sqlx::query("UPDATE books SET genres=ARRAY['changed-genre'] WHERE id=$1")
        .bind(base)
        .execute(owner)
        .await
        .unwrap();
    let old_group = store
        .books_report(&query(90, Some("even"), Some("flow-books-tag"), None))
        .await
        .unwrap();
    assert_eq!(old_group.matched_books, 4);
    assert_eq!(values(&old_group.current.totals), [None; 5]);
    let new_group = store
        .books_report(&query(90, Some("changed-genre"), None, None))
        .await
        .unwrap();
    assert_eq!(new_group.matched_books, 1);
    assert_eq!(new_group.current.totals.starts, Some(118 * 90));
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(base)
        .execute(owner)
        .await
        .unwrap();
    let hidden = store
        .books_report(&query(90, Some("changed-genre"), None, None))
        .await
        .unwrap();
    assert_eq!(hidden.matched_books, 0);
    assert!(hidden.rows.is_empty());
    assert_eq!(values(&hidden.current.totals), [None; 5]);
    assert!(!hidden.genres.iter().any(|value| value == "changed-genre"));
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn flow_books_empty_measurements_bounded_suggestions_and_health() {
    let fixture = Fixture::new("flow_books_empty", 9_065_000_300, 105).await;
    sqlx::query("UPDATE books SET genres=ARRAY['genre-'||(id-$1)::text],tags=ARRAY['tag-'||(id-$1)::text] WHERE id BETWEEN $1 AND $1+104")
        .bind(fixture.base).execute(&fixture.owner).await.unwrap();
    let store = PgDiscoveryAudience::new(fixture.app.clone());
    let report = store
        .books_report(&query(90, None, None, None))
        .await
        .unwrap();
    assert_eq!(report.matched_books, 105);
    assert_eq!(report.genres.len(), 100);
    assert_eq!(report.tags.len(), 100);
    assert!(report.filter_options_truncated);
    assert_eq!(values(&report.current.totals), [None; 5]);
    assert_eq!(report.current.counts.no_observations, 105);
    assert_eq!(report.current.counts.observed_without_starts, 0);
    assert_eq!(report.current.counts.with_starts, 0);
    let outside_suggestions = (0..105)
        .map(|n| format!("genre-{n}"))
        .find(|label| !report.genres.contains(label))
        .unwrap();
    let selected = store
        .books_report(&query(1, Some(&outside_suggestions), None, None))
        .await
        .unwrap();
    assert_eq!(
        selected.matched_books, 1,
        "suggestion truncation cannot restrict valid filters"
    );
    assert_eq!(selected.rows.len(), 1);
    let empty = store
        .books_report(&query(1, Some("no-such-genre"), None, None))
        .await
        .unwrap();
    assert_eq!(empty.matched_books, 0);
    assert!(empty.rows.is_empty());
    assert_eq!(empty.next_after_id, None);
    assert_eq!(empty.current.counts.no_observations, 0);
    assert_eq!(values(&empty.previous.totals), [None; 5]);
    sqlx::query(
        "INSERT INTO flow_books_empty.rec_worker_health(singleton,last_success) VALUES(true,now())",
    )
    .execute(&fixture.owner)
    .await
    .unwrap();
    sqlx::query("INSERT INTO flow_books_empty.rec_dirty_buckets(bucket_date,book_id) VALUES((now() AT TIME ZONE 'UTC')::date,$1)")
        .bind(fixture.base).execute(&fixture.owner).await.unwrap();
    sqlx::query("INSERT INTO flow_books_empty.organic_discovery_dirty(bucket_date,book_id) VALUES((now() AT TIME ZONE 'UTC')::date,$1)")
        .bind(fixture.base).execute(&fixture.owner).await.unwrap();
    let fresh = store
        .books_report(&query(1, None, None, None))
        .await
        .unwrap();
    assert!(fresh.health.fresh && fresh.health.rec_dirty && fresh.health.organic_dirty);
    assert!(fresh.health.last_success_utc.unwrap().ends_with('Z'));
    sqlx::query(
        "UPDATE flow_books_empty.rec_worker_health SET last_success=now()-interval '60 seconds'",
    )
    .execute(&fixture.owner)
    .await
    .unwrap();
    assert!(
        !store
            .books_report(&query(1, None, None, None))
            .await
            .unwrap()
            .health
            .fresh
    );
    fixture.app.close().await;
    assert_eq!(
        store.books_report(&query(1, None, None, None)).await,
        Err(DiscoveryError::Unavailable)
    );
    let mut invalid = query(1, None, None, None);
    invalid.after_id = Some(0);
    assert_eq!(
        store.books_report(&invalid).await,
        Err(DiscoveryError::Invalid)
    );
    fixture.finish().await;
}
