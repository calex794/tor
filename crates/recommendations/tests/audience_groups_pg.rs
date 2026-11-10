//! Real PostgreSQL: one taxonomy at a time over complete public-book totals.
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::collections::BTreeSet;
use tor_recommendations::{
    repositories::audience::PgDiscoveryAudience,
    services::audience::{
        books::{BookDiscoveryMetrics, BookDiscoveryQuery},
        groups::{
            GroupDiscoveryKind, GroupDiscoveryQuery, GroupDiscoveryReport, GroupDiscoveryRow,
        },
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
        sqlx::query("DELETE FROM books WHERE id BETWEEN $1 AND $1::bigint+199")
            .bind(base)
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books(id,source_identity,target_language,title,visibility,access_level,genres,tags)
             SELECT $1::bigint+i,'ci:flow:groups:'||($1::bigint+i)::text,'ru','Книга '||i,
                    'public','basic',ARRAY['all','g-'||lpad(i::text,3,'0')],
                    ARRAY['tag-all','t-'||lpad(i::text,3,'0')]
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
        sqlx::query("DELETE FROM books WHERE id BETWEEN $1 AND $1::bigint+199")
            .bind(self.base)
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

fn query(days: i32, kind: GroupDiscoveryKind, after: Option<String>) -> GroupDiscoveryQuery {
    GroupDiscoveryQuery::new(days, kind, after).unwrap()
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

async fn all_groups(
    store: &PgDiscoveryAudience,
    days: i32,
    kind: GroupDiscoveryKind,
) -> (GroupDiscoveryReport, Vec<GroupDiscoveryRow>) {
    let first = store.groups_report(&query(days, kind, None)).await.unwrap();
    let mut rows = first.rows.clone();
    let mut after = first.next_after.clone();
    let mut pages = 1;
    while let Some(cursor) = after {
        assert!(pages < 10, "cursor must make progress");
        let page = store
            .groups_report(&query(days, kind, Some(cursor)))
            .await
            .unwrap();
        assert_eq!(page.current, first.current);
        assert_eq!(page.previous, first.previous);
        assert_eq!(page.matched_groups, first.matched_groups);
        assert_eq!(page.matched_books, first.matched_books);
        assert!(page.rows.len() <= 20);
        rows.extend(page.rows);
        after = page.next_after;
        pages += 1;
    }
    assert_eq!(rows.len() as i64, first.matched_groups);
    let unique: BTreeSet<_> = rows.iter().map(|row| row.label.clone()).collect();
    assert_eq!(unique.len(), rows.len());
    assert_eq!(
        unique.into_iter().collect::<Vec<_>>(),
        rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>()
    );
    (first, rows)
}

async fn assert_detail(
    store: &PgDiscoveryAudience,
    days: i32,
    kind: GroupDiscoveryKind,
    row: &GroupDiscoveryRow,
) {
    let (genre, tag, no_genre, no_tag) = match kind {
        GroupDiscoveryKind::Genres => (row.label.clone(), None, row.label.is_none(), false),
        GroupDiscoveryKind::Tags => (None, row.label.clone(), false, row.label.is_none()),
    };
    let detail = store
        .books_report(
            &BookDiscoveryQuery::with_missing(days, genre, tag, no_genre, no_tag, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.matched_books, row.matched_books);
    assert_eq!(detail.current.totals, row.current);
    assert_eq!(detail.previous.totals, row.previous);
    assert_eq!(detail.current.counts, row.current_counts);
    assert_eq!(detail.previous.counts, row.previous_counts);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn flow_groups_full_ranges_dedup_paging_current_metadata_and_details() {
    let fixture = Fixture::new("flow_groups_totals", 9_066_000_000, 33).await;
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
        "UPDATE books SET genres=ARRAY['secret-genre'],tags=ARRAY['secret-tag'] WHERE id=$1",
    )
    .bind(base + 32)
    .execute(owner)
    .await
    .unwrap();
    sqlx::query("UPDATE books SET genres=ARRAY['all','focus','focus'],tags=ARRAY['tag-all',CASE WHEN id=$1 THEN 'left' ELSE 'right' END,CASE WHEN id=$1 THEN 'left' ELSE 'right' END] WHERE id BETWEEN $1 AND $1::bigint+1")
        .bind(base).execute(owner).await.unwrap();
    sqlx::query(
        "UPDATE books SET genres='{}',tags='{}' WHERE id BETWEEN $1::bigint+4 AND $1::bigint+5",
    )
    .bind(base)
    .execute(owner)
    .await
    .unwrap();
    sqlx::query("UPDATE books SET genres=ARRAY['all','a & /? + ж'],tags=ARRAY['tag-all','weird < tag'] WHERE id=$1::bigint+6")
        .bind(base).execute(owner).await.unwrap();
    sqlx::query(
        "INSERT INTO flow_groups_totals.rec_quality_daily
          (bucket_date,book_id,surface,source,reason,position,generation,algorithm_version,
           impressions,paired_clicks,unpaired_clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date-d,$1::bigint+b,
                'home','content','similar_by_content',1,'fallback','flow-groups-test',7,2,3,5,11,0
         FROM generate_series(0,179) d CROSS JOIN generate_series(0,1) b",
    )
    .bind(base)
    .execute(owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_groups_totals.organic_discovery_quality_daily
           (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date-d,$1::bigint+b,c.source,c.issued,c.clicks,c.opens,c.starts,0
         FROM generate_series(0,179) d CROSS JOIN generate_series(0,1) b
         CROSS JOIN (VALUES ('catalog',13,17,19,23),('search',29,31,37,41),
                            ('unattributed',0,0,0,43)) c(source,issued,clicks,opens,starts)",
    ).bind(base).execute(owner).await.unwrap();
    sqlx::query(
        "INSERT INTO flow_groups_totals.organic_discovery_quality_daily
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
        "INSERT INTO flow_groups_totals.organic_discovery_quality_daily
          (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         SELECT (now() AT TIME ZONE 'UTC')::date,$1::bigint+i,source,issued,0,0,starts,0
         FROM (VALUES (2,'catalog',2,0),(3,'catalog',0,0),(4,'unattributed',0,1)) v(i,source,issued,starts)",
    ).bind(base).execute(owner).await.unwrap();
    let store = PgDiscoveryAudience::new(fixture.app.clone());
    let before: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM rec_quality_daily),(SELECT count(*) FROM organic_discovery_quality_daily)")
        .fetch_one(&fixture.app).await.unwrap();
    assert!(before.0 > 100 && before.1 > 100);
    for days in DISCOVERY_PERIOD_DAYS {
        for kind in [GroupDiscoveryKind::Genres, GroupDiscoveryKind::Tags] {
            let (report, groups) = all_groups(&store, days, kind).await;
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
            assert_eq!(report.rows.len(), 20);
            assert!(report.next_after.is_some());
            assert!(report.current.includes_partial_today);
            assert!(!report.previous.includes_partial_today);
            assert_eq!(report.previous.until_day_exclusive, report.current.from_day);
            assert_eq!(report.current.counts.with_starts, 3);
            assert_eq!(report.current.counts.observed_without_starts, 1);
            assert_eq!(report.current.counts.no_observations, 22);
            assert!(!groups
                .iter()
                .any(|g| g.label.as_ref().is_some_and(|s| s.starts_with("secret-"))));
            let missing = &groups[0];
            assert_eq!(missing.label, None);
            assert_eq!(missing.matched_books, 2);
            assert_eq!(values(&missing.current), [None, None, None, None, Some(1)]);
            assert_eq!(missing.current_counts.with_starts, 1);
            assert_eq!(missing.current_counts.no_observations, 1);
            for group in &groups {
                assert_detail(&store, days, kind, group).await;
            }
            if kind == GroupDiscoveryKind::Genres {
                let focus = groups
                    .iter()
                    .find(|g| g.label.as_deref() == Some("focus"))
                    .unwrap();
                assert_eq!(focus.matched_books, 2);
                assert_eq!(
                    values(&focus.current),
                    [
                        Some(42 * n),
                        Some(7 * n),
                        Some(53 * n),
                        Some(61 * n),
                        Some(118 * n)
                    ]
                );
                assert_eq!(focus.current_counts.with_starts, 2);
            }
        }
    }
    let after: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM rec_quality_daily),(SELECT count(*) FROM organic_discovery_quality_daily)")
        .fetch_one(&fixture.app).await.unwrap();
    assert_eq!(
        before, after,
        "read-only reports cannot mutate retained facts"
    );
    // Legacy unknown REC opens poison only groups that include this book.
    sqlx::query("UPDATE flow_groups_totals.rec_quality_daily SET book_opens=NULL WHERE book_id=$1::bigint+1 AND bucket_date=(now() AT TIME ZONE 'UTC')::date-45")
        .bind(base).execute(owner).await.unwrap();
    let (incomplete, tags) = all_groups(&store, 90, GroupDiscoveryKind::Tags).await;
    assert_eq!(incomplete.current.totals.book_opens, None);
    assert_eq!(incomplete.previous.totals.book_opens, Some(61 * 180));
    for (label, expected) in [("left", Some(61 * 90)), ("right", None)] {
        let group = tags
            .iter()
            .find(|g| g.label.as_deref() == Some(label))
            .unwrap();
        assert_eq!(group.current.book_opens, expected);
        assert_detail(&store, 90, GroupDiscoveryKind::Tags, group).await;
    }
    assert_eq!(
        store
            .groups_report(&query(1, GroupDiscoveryKind::Genres, None))
            .await
            .unwrap()
            .current
            .totals
            .book_opens,
        Some(61 * 2)
    );
    // Current metadata relocates all retained facts; hidden books leave every group.
    sqlx::query("UPDATE books SET genres=ARRAY['changed'] WHERE id=$1")
        .bind(base)
        .execute(owner)
        .await
        .unwrap();
    let (_, moved) = all_groups(&store, 90, GroupDiscoveryKind::Genres).await;
    let changed = moved
        .iter()
        .find(|g| g.label.as_deref() == Some("changed"))
        .unwrap();
    assert_eq!(changed.matched_books, 1);
    assert_eq!(changed.current.starts, Some(118 * 90));
    assert_detail(&store, 90, GroupDiscoveryKind::Genres, changed).await;
    assert_eq!(
        moved
            .iter()
            .find(|g| g.label.as_deref() == Some("focus"))
            .unwrap()
            .matched_books,
        1
    );
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(base)
        .execute(owner)
        .await
        .unwrap();
    let (hidden, groups) = all_groups(&store, 90, GroupDiscoveryKind::Genres).await;
    assert_eq!(hidden.matched_books, 25);
    assert!(!groups.iter().any(|g| g.label.as_deref() == Some("changed")));
    assert_eq!(hidden.current.totals.starts, Some(118 * 90 + 1));
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn flow_groups_more_than_hundred_labels_missing_zero_health_and_unavailable() {
    let fixture = Fixture::new("flow_groups_empty", 9_066_000_300, 105).await;
    let owner = &fixture.owner;
    let base = fixture.base;
    sqlx::query("UPDATE books SET genres='{}',tags=ARRAY['tag-'||lpad((id-$1)::text,3,'0')] WHERE id BETWEEN $1 AND $1::bigint+104")
        .bind(base).execute(owner).await.unwrap();
    sqlx::query("INSERT INTO flow_groups_empty.organic_discovery_quality_daily(bucket_date,book_id,source,issued,clicks,book_opens,starts,completions) VALUES((now() AT TIME ZONE 'UTC')::date,$1,'catalog',0,0,0,0,0)")
        .bind(base).execute(owner).await.unwrap();
    let store = PgDiscoveryAudience::new(fixture.app.clone());
    let (report, tags) = all_groups(&store, 90, GroupDiscoveryKind::Tags).await;
    assert_eq!(report.matched_groups, 105);
    assert_eq!(report.matched_books, 105);
    assert_eq!(
        values(&tags[0].current),
        [Some(0), None, Some(0), Some(0), Some(0)]
    );
    assert_eq!(values(&tags[1].current), [None; 5]);
    assert_eq!(report.current.counts.no_observations, 105);
    assert_eq!(report.current.counts.with_starts, 0);
    let genres = store
        .groups_report(&query(90, GroupDiscoveryKind::Genres, None))
        .await
        .unwrap();
    assert_eq!(genres.rows.len(), 1);
    assert_eq!(genres.rows[0].label, None);
    assert_eq!(genres.rows[0].matched_books, 105);
    assert_eq!(genres.rows[0].current, report.current.totals);
    assert_eq!(genres.next_after, None);
    assert_detail(&store, 90, GroupDiscoveryKind::Genres, &genres.rows[0]).await;
    let missing_tag = store
        .books_report(&BookDiscoveryQuery::with_missing(90, None, None, true, true, None).unwrap())
        .await
        .unwrap();
    assert_eq!(missing_tag.matched_books, 0);
    assert_eq!(values(&missing_tag.current.totals), [None; 5]);
    sqlx::query("INSERT INTO flow_groups_empty.rec_worker_health(singleton,last_success) VALUES(true,now())").execute(owner).await.unwrap();
    for table in ["rec_dirty_buckets", "organic_discovery_dirty"] {
        sqlx::query(&format!("INSERT INTO flow_groups_empty.{table}(bucket_date,book_id) VALUES((now() AT TIME ZONE 'UTC')::date,$1)"))
            .bind(base).execute(owner).await.unwrap();
    }
    let fresh = store
        .groups_report(&query(1, GroupDiscoveryKind::Genres, None))
        .await
        .unwrap();
    assert!(fresh.health.fresh && fresh.health.rec_dirty && fresh.health.organic_dirty);
    assert!(fresh.health.last_success_utc.unwrap().ends_with('Z'));
    sqlx::query(
        "UPDATE flow_groups_empty.rec_worker_health SET last_success=now()-interval '60 seconds'",
    )
    .execute(owner)
    .await
    .unwrap();
    assert!(
        !store
            .groups_report(&query(1, GroupDiscoveryKind::Genres, None))
            .await
            .unwrap()
            .health
            .fresh
    );
    sqlx::query("UPDATE books SET visibility='draft' WHERE id BETWEEN $1 AND $1::bigint+104")
        .bind(base)
        .execute(owner)
        .await
        .unwrap();
    let empty = store
        .groups_report(&query(1, GroupDiscoveryKind::Genres, None))
        .await
        .unwrap();
    assert!(empty.rows.is_empty());
    assert_eq!(empty.matched_groups, 0);
    assert_eq!(empty.matched_books, 0);
    assert_eq!(empty.current.counts.no_observations, 0);
    assert_eq!(values(&empty.current.totals), [None; 5]);
    fixture.app.close().await;
    assert_eq!(
        store
            .groups_report(&query(1, GroupDiscoveryKind::Genres, None))
            .await,
        Err(DiscoveryError::Unavailable)
    );
    let mut invalid = query(1, GroupDiscoveryKind::Tags, None);
    invalid.after = Some(" ".into());
    assert_eq!(
        store.groups_report(&invalid).await,
        Err(DiscoveryError::Invalid)
    );
    fixture.finish().await;
}
