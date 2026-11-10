//! Real PostgreSQL: current groups sum complete bounded same-book observations.
//! Synthetic dates prove the report contract; production has no group collector.
#[path = "audience_group_returns_pg/fixtures.rs"]
mod fixtures;

use fixtures::Fixture;
use tor_reading::{
    repositories::returns::PgReadingReturns,
    services::{
        book_returns::BookReadingReturnsPeriod,
        group_book_returns::{
            GroupBookReadingReturnsKind as Kind, GroupBookReadingReturnsQuery as Query,
            GroupBookReadingReturnsReport as Report,
        },
        returns::ReadingReturnsError,
    },
};

fn query(days: i32, kind: Kind, label: Option<&str>) -> Query {
    Query::new(days, kind, label.map(str::to_owned)).unwrap()
}

async fn group(store: &PgReadingReturns, days: i32, kind: Kind, label: Option<&str>) -> Report {
    store
        .group_book_returns_report(&query(days, kind, label))
        .await
        .unwrap()
}

fn metric_values(period: &BookReadingReturnsPeriod) -> [[i64; 4]; 2] {
    assert_eq!(period.metrics.map(|metric| metric.day), [7, 30]);
    period.metrics.map(|metric| {
        [
            metric.eligible,
            metric.returned,
            metric.pending,
            metric.unknown,
        ]
    })
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn flow_group_book_returns_current_membership_exact_days_and_read_only() {
    let fixture = Fixture::new().await;
    let store = PgReadingReturns::new(fixture.app.clone());
    let writable: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('reader_book_return_subjects','INSERT,UPDATE,DELETE')
             OR has_table_privilege('reader_book_return_daily','INSERT,UPDATE,DELETE')
             OR has_table_privilege('reader_book_return_coverage','INSERT,UPDATE,DELETE')",
    )
    .fetch_one(&fixture.app)
    .await
    .unwrap();
    assert!(!writable, "the report runs with SELECT-only fixture grants");
    empty_and_validation(&fixture, &store).await;
    complete_subsets(&fixture, &store).await;
    current_membership_and_visibility(&fixture, &store).await;
    exact_periods_and_gaps(&fixture, &store).await;
    fixture.finish().await;
}

async fn empty_and_validation(fixture: &Fixture, store: &PgReadingReturns) {
    let before = fixture.fingerprint().await;
    let empty = group(store, 90, Kind::Genres, Some("not-present")).await;
    assert_eq!(empty.matched_books, 0);
    assert_eq!(empty.current.enrolled, 0);
    assert_eq!(empty.observation_started_at_utc, None);
    assert!(!empty.collection_healthy && !empty.current.coverage_complete);
    assert_eq!(metric_values(&empty.current), [[0; 4]; 2]);
    let present = group(store, 90, Kind::Genres, Some(" ALL ")).await;
    assert_eq!(present.query.label.as_deref(), Some("all"));
    assert_eq!(present.matched_books, 29);
    assert_eq!(present.current.enrolled, 0);
    for forged in [
        Query {
            days: 2,
            kind: Kind::Genres,
            label: Some("all".into()),
        },
        Query {
            days: 90,
            kind: Kind::Tags,
            label: Some(" ".into()),
        },
        Query {
            days: 90,
            kind: Kind::Genres,
            label: Some("x\ny".into()),
        },
    ] {
        assert_eq!(
            store.group_book_returns_report(&forged).await,
            Err(ReadingReturnsError::Invalid)
        );
    }
    assert_eq!(
        fixture.fingerprint().await,
        before,
        "reading creates no coverage or observations"
    );
}

fn weight(index: usize) -> i64 {
    if index == 1 {
        9
    } else {
        index as i64 + 1
    }
}

async fn complete_subsets(fixture: &Fixture, store: &PgReadingReturns) {
    fixture
        .metadata(
            0,
            &["all", "all", "overlap"],
            &["tag-all", "tag-all", "side"],
        )
        .await;
    fixture.metadata(1, &["all", "overlap"], &["side"]).await;
    fixture.metadata(2, &[], &["tag-all"]).await;
    fixture.metadata(3, &["all"], &[]).await;
    fixture.metadata(4, &[], &[]).await;
    fixture.healthy_coverage().await;
    for index in 0..fixture.books.len() {
        let n = weight(index);
        fixture.daily(index, 32, [n, n, 1, 0]).await;
        fixture.daily(index, 90, [2 * n, 2 * n - 1, 1, 1]).await;
        fixture.daily(index, 179, [3, 2, 1, 1]).await;
        fixture.daily(index, 180, [99, 99, 99, 0]).await;
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM reader_book_return_daily")
        .fetch_one(&fixture.app)
        .await
        .unwrap();
    assert!(rows > 100);
    let before = fixture.fingerprint().await;
    let subsets = [
        (
            Kind::Genres,
            Some("all"),
            (0..29)
                .filter(|index| ![2, 4].contains(index))
                .collect::<Vec<_>>(),
        ),
        (
            Kind::Tags,
            Some("tag-all"),
            (0..29).filter(|index| ![1, 3, 4].contains(index)).collect(),
        ),
        (Kind::Genres, Some("overlap"), vec![0, 1]),
        (Kind::Tags, Some("side"), vec![0, 1]),
        (Kind::Genres, None, vec![2, 4]),
        (Kind::Tags, None, vec![3, 4]),
    ];
    for (kind, label, indices) in subsets {
        let result = group(store, 90, kind, label).await;
        let count = indices.len() as i64;
        let total: i64 = indices.into_iter().map(weight).sum();
        assert_eq!(result.matched_books, count);
        assert_eq!(
            result.current.enrolled, total,
            "D7 and D30 never double the cohort"
        );
        assert_eq!(metric_values(&result.current), [[total, count, 0, 0]; 2]);
        assert_eq!(result.previous.enrolled, 2 * total + 3 * count);
        assert_eq!(
            metric_values(&result.previous),
            [[2 * total + count, 2 * count, 0, 2 * count]; 2]
        );
        assert!(
            result.collection_healthy
                && result.current.coverage_complete
                && result.previous.coverage_complete
        );
    }
    let overlap = group(store, 90, Kind::Genres, Some("overlap")).await;
    assert_eq!(
        metric_values(&overlap.current),
        [[10, 2, 0, 0]; 2],
        "sum 1/1 and 1/9 as 2/10, never average their percentages"
    );
    let empty = group(store, 90, Kind::Tags, Some("absent")).await;
    assert_eq!(empty.matched_books, 0);
    assert_eq!(empty.current.enrolled, 0);
    assert!(empty.collection_healthy && empty.current.coverage_complete);
    assert_eq!(
        fixture.fingerprint().await,
        before,
        "reports cannot finalize, refresh or collect data"
    );
}

async fn current_membership_and_visibility(fixture: &Fixture, store: &PgReadingReturns) {
    let before = fixture.fingerprint().await;
    fixture.metadata(0, &["renamed"], &["new-tag"]).await;
    sqlx::query("UPDATE public.books SET title='Новое название' WHERE id=$1")
        .bind(fixture.books[0])
        .execute(&fixture.owner)
        .await
        .unwrap();
    let moved = group(store, 90, Kind::Genres, Some("renamed")).await;
    assert_eq!(
        (
            moved.matched_books,
            moved.current.enrolled,
            moved.previous.enrolled
        ),
        (1, 1, 5)
    );
    let old = group(store, 90, Kind::Genres, Some("overlap")).await;
    assert_eq!((old.matched_books, old.current.enrolled), (1, 9));
    let by_tag = group(store, 90, Kind::Tags, Some("new-tag")).await;
    assert_eq!(by_tag.current, moved.current);
    let book = store
        .book_returns_report(fixture.books[0], 90)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(book.book_title, "Новое название");
    assert_eq!(book.current, moved.current);
    assert_eq!(book.previous, moved.previous);

    for (hide, restore) in [
        ("visibility='draft'", "visibility='public'"),
        ("visibility_status='hidden'", "visibility_status='active'"),
        (
            "visibility_status='pending_moderation'",
            "visibility_status='active'",
        ),
        ("visibility_status='blocked'", "visibility_status='active'"),
        ("admin_hold=true", "admin_hold=false"),
        ("is_deleted=true", "is_deleted=false"),
        (
            "lifecycle_status='pending_deletion'",
            "lifecycle_status='in_progress'",
        ),
        ("access_level='adult'", "access_level='basic'"),
        ("access_level='pro_adult'", "access_level='basic'"),
    ] {
        sqlx::query(&format!("UPDATE public.books SET {hide} WHERE id=$1"))
            .bind(fixture.books[0])
            .execute(&fixture.owner)
            .await
            .unwrap();
        assert_hidden(fixture, store).await;
        sqlx::query(&format!("UPDATE public.books SET {restore} WHERE id=$1"))
            .bind(fixture.books[0])
            .execute(&fixture.owner)
            .await
            .unwrap();
    }
    for audience in ["adult", "unreviewed"] {
        sqlx::query("INSERT INTO public.book_recommendation_audiences(book_id,audience) VALUES($1,$2) ON CONFLICT(book_id) DO UPDATE SET audience=$2")
            .bind(fixture.books[0]).bind(audience).execute(&fixture.owner).await.unwrap();
        assert_hidden(fixture, store).await;
    }
    sqlx::query(
        "UPDATE public.book_recommendation_audiences SET audience='general' WHERE book_id=$1",
    )
    .bind(fixture.books[0])
    .execute(&fixture.owner)
    .await
    .unwrap();
    assert_eq!(
        group(store, 90, Kind::Genres, Some("renamed"))
            .await
            .current,
        moved.current
    );
    fixture.metadata(0, &[], &[]).await;
    let vanished = group(store, 90, Kind::Genres, Some("renamed")).await;
    assert_eq!((vanished.matched_books, vanished.current.enrolled), (0, 0));
    assert!(vanished.current.coverage_complete);
    assert_eq!(
        fixture.fingerprint().await,
        before,
        "current metadata never rewrites observations"
    );
}

async fn assert_hidden(fixture: &Fixture, store: &PgReadingReturns) {
    assert!(store
        .book_returns_report(fixture.books[0], 90)
        .await
        .unwrap()
        .is_none());
    for (kind, label) in [(Kind::Genres, "renamed"), (Kind::Tags, "new-tag")] {
        let hidden = group(store, 90, kind, Some(label)).await;
        assert_eq!(
            (
                hidden.matched_books,
                hidden.current.enrolled,
                hidden.previous.enrolled
            ),
            (0, 0, 0)
        );
        assert_eq!(metric_values(&hidden.current), [[0; 4]; 2]);
    }
}

async fn exact_periods_and_gaps(fixture: &Fixture, store: &PgReadingReturns) {
    sqlx::query("DELETE FROM reader_book_return_daily")
        .execute(&fixture.owner)
        .await
        .unwrap();
    fixture.metadata(0, &["temporal"], &["clock"]).await;
    fixture.metadata(1, &["temporal"], &["clock"]).await;
    fixture.healthy_coverage().await;
    for age in [0, 1, 6, 7, 8, 29, 30, 31] {
        let subject = format!("00000000-0000-0000-0000-{age:012}");
        fixture.cohort(0, age, &subject, [true, true]).await;
    }
    // The same account in another book adds an observation, not a return to
    // the first book; the D7 numerator comes only from that book's own flag.
    fixture
        .cohort(1, 8, "00000000-0000-0000-0000-000000000008", [false, false])
        .await;
    for age in [45, 89, 90, 179, 180] {
        fixture.daily(0, age, [2, 1, 1, 1]).await;
    }
    let before = fixture.fingerprint().await;
    for (days, enrolled, current, previous) in [
        (1, [1, 1], [[0, 0, 1, 0]; 2], [[0, 0, 1, 0]; 2]),
        (7, [3, 3], [[0, 0, 3, 0]; 2], [[2, 1, 1, 0], [0, 0, 3, 0]]),
        (
            30,
            [7, 4],
            [[3, 2, 4, 0], [0, 0, 7, 0]],
            [[3, 3, 0, 1], [2, 2, 1, 1]],
        ),
        (90, [13, 4], [[7, 6, 4, 2], [3, 3, 8, 2]], [[2, 2, 0, 2]; 2]),
    ] {
        let result = group(store, days, Kind::Genres, Some("temporal")).await;
        assert_eq!(result.matched_books, 2);
        assert_eq!(
            [result.current.enrolled, result.previous.enrolled],
            enrolled,
            "period {days}"
        );
        assert_eq!(
            metric_values(&result.current),
            current,
            "current period {days}"
        );
        assert_eq!(
            metric_values(&result.previous),
            previous,
            "previous period {days}"
        );
        let tagged = group(store, days, Kind::Tags, Some("clock")).await;
        assert_eq!(tagged.current, result.current);
        assert_eq!(tagged.previous, result.previous);
    }
    assert_eq!(fixture.fingerprint().await, before);

    sqlx::query("DELETE FROM reader_book_return_coverage")
        .execute(&fixture.owner)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at,ended_at)
         VALUES(((now() AT TIME ZONE 'UTC')::date-181)::timestamp AT TIME ZONE 'UTC',
                ((now() AT TIME ZONE 'UTC')::date-1)::timestamp AT TIME ZONE 'UTC',
                ((now() AT TIME ZONE 'UTC')::date-1)::timestamp AT TIME ZONE 'UTC'),
               ((now() AT TIME ZONE 'UTC')::date::timestamp AT TIME ZONE 'UTC',now(),NULL)",
    )
    .execute(&fixture.owner)
    .await
    .unwrap();
    let gap = group(store, 90, Kind::Genres, Some("temporal")).await;
    assert!(gap.collection_healthy && !gap.current.coverage_complete);
    assert_eq!(gap.current.enrolled, 13);
    assert_eq!(
        metric_values(&gap.current),
        [[5, 5, 4, 4], [2, 2, 8, 3]],
        "yesterday's missing coverage stays unknown"
    );
    sqlx::query(
        "UPDATE reader_book_return_coverage SET ended_at=last_heartbeat_at WHERE ended_at IS NULL",
    )
    .execute(&fixture.owner)
    .await
    .unwrap();
    let stopped = group(store, 90, Kind::Genres, Some("temporal")).await;
    assert!(!stopped.collection_healthy);
    assert_eq!(
        metric_values(&stopped.current),
        [[5, 5, 3, 5], [2, 2, 7, 4]],
        "today's interrupted target is unknown, future targets are pending"
    );
}
