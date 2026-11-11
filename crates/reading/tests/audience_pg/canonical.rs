//! Exact migrated C tables, real SELECT-only role, no analytics or consent tables.
#[path = "canonical/fixture.rs"]
mod fixture;
use fixture::Fixture;
use sqlx::Row;
use tor_reading::{
    repositories::audience::PgAudience,
    services::audience::{
        canonical::{CanonicalDimension as Dim, CanonicalQuery as Query},
        AudienceError,
    },
};

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn retained_firsts_require_matching_c_evidence_and_never_count_later_chapters() {
    let f = Fixture::new().await;
    f.books(3).await;
    let r = PgAudience::new(f.app.clone());
    assert_eq!(
        r.canonical_report(&Query::new(7, Dim::Books))
            .await
            .unwrap()
            .totals
            .current_pairs,
        0
    );
    f.completion(1, 1, 11, "now()-interval '200 days'", "free")
        .await;
    f.completion(1, 1, 12, "now()-interval '1 hour'", "free")
        .await; // not a new pair
    f.completion(2, 1, 21, "now()-interval '200 days'", "invalid")
        .await;
    f.completion(2, 1, 22, "now()-interval '1 hour'", "free")
        .await; // invalid early row cannot preempt it
    f.completion(3, 2, 31, "now()-interval '1 hour'", "paid")
        .await;
    f.completion(4, 2, 41, "now()-interval '1 hour'", "mismatch")
        .await;
    f.completion(5, 2, 51, "now()-interval '1 hour'", "missing")
        .await;
    f.completion(6, 3, 61, "now()+interval '1 day'", "free")
        .await;
    f.completion(7, 3, 71, "now()-interval '8 days'", "free")
        .await;
    f.completion(8, 3, 81, "now()-interval '40 days'", "free")
        .await;
    // One account legitimately contributes to two books, not two unique humans.
    f.completion(3, 1, 32, "now()-interval '1 hour'", "free")
        .await;
    for (days, current, previous) in [(1, 3, 0), (7, 3, 1), (30, 4, 1), (90, 5, 0)] {
        let report = r
            .canonical_report(&Query::new(days, Dim::Books))
            .await
            .unwrap();
        assert_eq!(report.totals.books, 3);
        assert_eq!(report.totals.current_pairs, current);
        assert_eq!(report.totals.previous_pairs, previous);
        let at = chrono::DateTime::parse_from_rfc3339(&report.measured_at_utc).unwrap();
        let from = chrono::DateTime::parse_from_rfc3339(&report.current_from_utc).unwrap();
        assert_eq!((at - from).num_seconds(), i64::from(days) * 86400);
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn overlapping_labels_missing_groups_filters_and_keysets_do_not_multiply_totals() {
    let f = Fixture::new().await;
    f.books(43).await;
    f.completion(1, 1, 1, "now()-interval '1 hour'", "free")
        .await;
    f.completion(2, 2, 2, "now()-interval '1 hour'", "free")
        .await;
    f.sql("INSERT INTO reader_library_items(user_id,book_id) VALUES(1,1),(1,2),(2,3); INSERT INTO reader_book_follows(user_id,book_id) VALUES(1,1),(2,2)").await;
    let r = PgAudience::new(f.app.clone());
    let mut q = Query::new(7, Dim::Books);
    let first = r.canonical_report(&q).await.unwrap();
    assert_eq!(first.rows.len(), 20);
    assert_eq!(first.next_after_id, Some(20));
    q.after_id = first.next_after_id;
    let second = r.canonical_report(&q).await.unwrap();
    assert_eq!(second.totals, first.totals);
    assert_eq!(second.rows[0].book_id, Some(21));
    q.after_id = second.next_after_id;
    let last = r.canonical_report(&q).await.unwrap();
    assert_eq!(last.rows.len(), 3);
    assert_eq!(last.next_after_id, None);
    assert_eq!(first.totals.current_pairs, 2);
    assert_eq!(first.totals.library_links, 3);
    assert_eq!(first.totals.follow_links, 2);
    let groups = r
        .canonical_report(&Query::new(7, Dim::Genres))
        .await
        .unwrap();
    assert_eq!(groups.totals, first.totals);
    assert_eq!(groups.rows.len(), 3);
    assert_eq!(groups.rows[0].label, None);
    assert_eq!(groups.rows[1].label.as_deref(), Some("a"));
    assert_eq!(groups.rows[1].counts.current_pairs, 2);
    assert_eq!(groups.rows[2].label.as_deref(), Some("b"));
    assert_eq!(groups.rows[2].counts.current_pairs, 1);
    let mut filters = Query::new(7, Dim::Genres);
    filters.genre = Some(" B ".into());
    filters.tag = Some("x".into());
    assert_eq!(r.canonical_report(&filters).await.unwrap().totals.books, 1);
    filters.genre = None;
    filters.tag = None;
    filters.without_genre = true;
    assert_eq!(r.canonical_report(&filters).await.unwrap().totals.books, 1);
    f.sql("DELETE FROM reader_library_items WHERE user_id=1 AND book_id=1; DELETE FROM reader_book_follows WHERE user_id=1 AND book_id=1").await;
    assert_eq!(
        r.canonical_report(&Query::new(7, Dim::Books))
            .await
            .unwrap()
            .totals
            .library_links,
        2
    );
    f.sql("INSERT INTO reader_library_items(user_id,book_id) VALUES(1,1)")
        .await;
    assert_eq!(
        r.canonical_report(&Query::new(7, Dim::Books))
            .await
            .unwrap()
            .totals
            .library_links,
        3
    );
    f.sql("UPDATE reader_return_public_books SET tags=ARRAY['tag-'||lpad(id::text,3,'0')]")
        .await;
    let mut tags = Query::new(7, Dim::Tags);
    let first = r.canonical_report(&tags).await.unwrap();
    assert_eq!(first.next_after_label.as_deref(), Some("tag-020"));
    tags.after_label = first.next_after_label;
    assert_eq!(
        r.canonical_report(&tags).await.unwrap().rows[0]
            .label
            .as_deref(),
        Some("tag-021")
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn half_open_boundaries_and_each_locked_source_recover_without_writes() {
    let f = Fixture::new().await;
    f.books(1).await;
    for (user, at) in [
        (1, "'2026-11-09T12:00:00Z'"),
        (2, "'2026-11-10T12:00:00Z'"),
        (3, "'2026-11-11T11:59:59Z'"),
        (4, "'2026-11-11T12:00:00Z'"),
        (5, "'2026-11-09T11:59:59Z'"),
    ] {
        f.completion(user, 1, user, at, "free").await;
    }
    let sql = include_str!("../../src/repositories/audience/canonical/report.sql").replace(
        "transaction_timestamp()",
        "TIMESTAMPTZ '2026-11-11T12:00:00Z'",
    );
    let rows = sqlx::query(&sql)
        .bind(1_i32)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(false)
        .bind(false)
        .bind("books")
        .bind(None::<i64>)
        .bind(None::<String>)
        .bind(21_i64)
        .fetch_all(&f.app)
        .await
        .unwrap();
    assert_eq!(rows[0].get::<i64, _>("current_pairs"), 2);
    assert_eq!(rows[0].get::<i64, _>("previous_pairs"), 1);
    assert!(sqlx::query("DELETE FROM reader_chapter_completions")
        .execute(&f.app)
        .await
        .is_err());
    let r = PgAudience::new(f.app.clone());
    for table in [
        "reader_return_public_books",
        "reader_chapter_completions",
        "outbox_events",
        "reader_library_items",
        "reader_book_follows",
    ] {
        let mut lock = f.owner.begin().await.unwrap();
        sqlx::query(&format!(
            "LOCK TABLE {}.{table} IN ACCESS EXCLUSIVE MODE",
            f.schema
        ))
        .execute(&mut *lock)
        .await
        .unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            r.canonical_report(&Query::new(7, Dim::Books)).await,
            Err(AudienceError::Unavailable)
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        lock.rollback().await.unwrap();
        assert_eq!(
            r.canonical_report(&Query::new(7, Dim::Books))
                .await
                .unwrap()
                .totals
                .books,
            1
        );
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn one_hundred_thousand_facts_exclude_older_firsts_with_bounded_query() {
    let f = Fixture::new().await;
    f.books(100).await;
    f.sql("WITH facts AS (INSERT INTO reader_chapter_completions(user_id,book_id,chapter_id,first_version_id,completed_at,source_event_id) SELECT (n+1)/2,1+((n-1)/2)%100,n,n,CASE WHEN n%4=1 THEN now()-interval '200 days' ELSE now()-interval '1 hour' END,gen_random_uuid() FROM generate_series(1,100000)n RETURNING *) INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers) SELECT source_event_id,'ChapterCompletedV1',1,'chapter',chapter_id,source_event_id::text,jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,'version_id',first_version_id,'authority','C-verified-free-position'),ARRAY['fixture-only'] FROM facts").await;
    for table in ["reader_library_items", "reader_book_follows"] {
        f.sql(&format!(
            "INSERT INTO {table}(user_id,book_id) SELECT n,1+n%100 FROM generate_series(1,100000)n"
        ))
        .await;
    }
    f.sql("ANALYZE reader_chapter_completions; ANALYZE outbox_events; ANALYZE reader_return_public_books; ANALYZE reader_library_items; ANALYZE reader_book_follows").await;
    let r = PgAudience::new(f.app.clone());
    for dimension in [Dim::Books, Dim::Genres, Dim::Tags] {
        let start = std::time::Instant::now();
        let report = r
            .canonical_report(&Query::new(90, dimension))
            .await
            .unwrap();
        eprintln!("canonical100k {:?}: {:?}", dimension, start.elapsed());
        assert_eq!(report.totals.current_pairs, 25_000);
        assert_eq!(report.totals.previous_pairs, 0);
        assert_eq!(report.totals.library_links, 100_000);
        assert_eq!(report.totals.follow_links, 100_000);
    }
    f.finish().await;
}
