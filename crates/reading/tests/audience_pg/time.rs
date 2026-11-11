//! Actual C read port with an isolated, fixed database clock and SELECT-only role.
#[path = "time/fixture.rs"]
mod fixture;

use fixture::{at, counts, event, fixture};
use tor_reading::{
    repositories::audience::PgAudience,
    services::audience::{time::ReadingTimePeriod, AudienceError},
};

fn totals_match(period: &ReadingTimePeriod, expected: (i64, i64)) {
    assert_eq!(period.totals, counts(expected.0, expected.1));
    let sum = period
        .hours
        .iter()
        .map(|h| h.counts.unwrap())
        .fold((0, 0), |(s, c), row| {
            (s + row.starts, c + row.chapter_completions)
        });
    assert_eq!(sum, expected);
    for cells in [&period.by_hour[..], &period.by_weekday[..]] {
        let mut sum = (0, 0);
        let mut hours = 0;
        for cell in cells {
            hours += cell.utc_hours;
            if cell.utc_hours == 0 {
                assert_eq!(cell.counts, None);
            } else {
                let value = cell.counts.unwrap();
                sum.0 += value.starts;
                sum.1 += value.chapter_completions;
            }
        }
        assert_eq!(hours as usize, period.hours.len());
        assert_eq!(sum, expected);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn equal_closed_windows_include_every_new_chapter_and_exact_boundaries() {
    let f = fixture("2026-11-11T12:34:56Z").await;
    let repo = PgAudience::new(f.app.clone());
    let empty = repo.time_report(1, "UTC").await.unwrap();
    let anchor = chrono::DateTime::parse_from_rfc3339("2026-11-11T12:00:00Z")
        .unwrap()
        .timestamp();
    assert_eq!(empty.until_epoch, anchor);
    assert_eq!(empty.current.totals, counts(0, 0));
    assert_eq!(empty.previous.totals, counts(0, 0));
    for days in [1, 7, 30, 90] {
        f.sql("DELETE FROM reader_chapter_starts; DELETE FROM reader_chapter_completions; DELETE FROM outbox_events").await;
        let span = i64::from(days) * 86400;
        for (index, delta) in [-2 * span - 1, -2 * span, -span, -1, 0]
            .into_iter()
            .enumerate()
        {
            for finish in [false, true] {
                event(&f, index as i64 + 1, &at(anchor + delta), finish, "free").await;
            }
        }
        // Same old account/book, another chapter: this is not the first-book metric.
        for finish in [false, true] {
            event(&f, 100, &at(anchor - 200 * 86400), finish, "free").await;
            event(&f, 101, &at(anchor - 1800), finish, "paid").await;
            event(&f, 101, &at(anchor - 900), finish, "paid").await; // replay adds nothing
        }
        let report = repo.time_report(days, "UTC").await.unwrap();
        assert_eq!(report.until_epoch, anchor);
        assert_eq!(report.current.until_epoch, anchor);
        assert_eq!(report.current.from_epoch, anchor - span);
        assert_eq!(report.previous.until_epoch, anchor - span);
        assert_eq!(report.previous.from_epoch, anchor - 2 * span);
        assert_eq!(report.current.from_utc, report.previous.until_utc);
        assert_eq!(report.current.hours.len(), days as usize * 24);
        assert_eq!(report.previous.hours.len(), days as usize * 24);
        totals_match(&report.current, (3, 3));
        totals_match(&report.previous, (1, 1));
        for period in [&report.current, &report.previous] {
            assert_eq!(period.until_epoch - period.from_epoch, span);
            assert_eq!(period.hours.first().unwrap().epoch, period.from_epoch);
            assert_eq!(
                period.hours.last().unwrap().epoch,
                period.until_epoch - 3600
            );
        }
        for private in [
            "user_id",
            "book_id",
            "chapter_id",
            "purchase_id",
            "source_event_id",
        ] {
            assert!(!format!("{report:?}").contains(private));
        }
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn one_unmatched_retained_fact_taints_its_hour_and_every_containing_sum() {
    let f = fixture("2026-11-11T12:34:56Z").await;
    let repo = PgAudience::new(f.app.clone());
    let anchor = repo.time_report(7, "UTC").await.unwrap().until_epoch;
    event(&f, 1, &at(anchor - 3600), true, "missing").await;
    event(&f, 2, &at(anchor - 3600), false, "free").await;
    event(&f, 3, &at(anchor - 90000), true, "free").await; // same local hour on another day
    for finish in [false, true] {
        event(&f, 4, &at(anchor - 7200), finish, "paid").await;
    }
    event(&f, 5, &at(anchor - 10800), false, "invalid").await;
    event(&f, 6, &at(anchor - 14400), true, "mismatch").await;
    event(&f, 7, &at(anchor - 8 * 86400), true, "missing").await;
    // Outside the selected windows, bad evidence cannot make a good slot unknown.
    event(&f, 8, &at(anchor + 3600), true, "missing").await;
    event(&f, 9, &at(anchor - 181 * 86400), false, "invalid").await;
    let report = repo.time_report(7, "UTC").await.unwrap();
    assert_eq!(report.current.totals, None);
    assert_eq!(report.previous.totals, None);
    let latest = report.current.hours.last().unwrap();
    assert_eq!(latest.counts, None);
    assert_eq!(report.current.by_hour[latest.local_hour].utc_hours, 7);
    assert_eq!(report.current.by_hour[latest.local_hour].counts, None);
    assert_eq!(report.current.by_weekday[latest.weekday].counts, None);
    let good = report
        .current
        .hours
        .iter()
        .find(|h| h.epoch == anchor - 7200)
        .unwrap();
    assert_eq!(good.counts, counts(1, 1));
    assert_eq!(report.current.by_hour[good.local_hour].counts, counts(1, 1));
    for delta in [10800, 14400] {
        assert_eq!(
            report
                .current
                .hours
                .iter()
                .find(|h| h.epoch == anchor - delta)
                .unwrap()
                .counts,
            None
        );
    }
    assert!(report
        .current
        .hours
        .iter()
        .any(|h| h.counts == counts(0, 0)));
    // Restoring one correct proof does not conceal the other invalid hours.
    f.sql("WITH c AS (SELECT * FROM reader_chapter_completions WHERE chapter_id=1)
        INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
        SELECT source_event_id,'ChapterCompletedV1',1,'chapter',chapter_id,source_event_id::text,
        jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,'version_id',first_version_id,'authority','C-verified-free-position'),
        ARRAY['fixture-only'] FROM c").await;
    let repaired = repo.time_report(7, "UTC").await.unwrap();
    assert_eq!(repaired.current.hours.last().unwrap().counts, counts(1, 1));
    assert_eq!(
        repaired.current.by_hour[latest.local_hour].counts,
        counts(1, 2)
    );
    assert_eq!(repaired.current.totals, None);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn spring_dst_has_no_invented_local_hour_and_keeps_twenty_four_real_hours() {
    let f = fixture("2026-03-30T00:05:00Z").await;
    event(&f, 1, "'2026-03-29T00:59:59.999999Z'", false, "free").await;
    event(&f, 2, "'2026-03-29T01:00:00Z'", true, "free").await;
    let report = PgAudience::new(f.app.clone())
        .time_report(1, "Europe/Amsterdam")
        .await
        .unwrap();
    totals_match(&report.current, (1, 1));
    assert_eq!(report.current.hours.len(), 24);
    assert!(report
        .current
        .hours
        .iter()
        .all(|h| h.local_label != "2026-03-29 02:00"));
    assert_eq!(report.current.by_hour[2].utc_hours, 0);
    assert_eq!(report.current.by_hour[2].counts, None);
    assert_eq!(report.current.by_hour[1].utc_hours, 2);
    assert_eq!(report.current.by_hour[1].counts, counts(1, 0));
    assert_eq!(report.current.by_hour[3].counts, counts(0, 1));
    assert_eq!(report.current.by_weekday[6].utc_hours, 22);
    assert_eq!(report.current.by_weekday[0].utc_hours, 2);
    assert_eq!(report.current.hours[0].offset_minutes, 60);
    assert_eq!(report.current.hours[1].offset_minutes, 120);
    assert_eq!(
        report.current.hours[1].epoch - report.current.hours[0].epoch,
        3600
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn autumn_dst_retains_both_offsets_and_does_not_double_or_lose_any_fact() {
    let f = fixture("2026-10-26T00:05:00Z").await;
    event(&f, 1, "'2026-10-25T00:59:59.999999Z'", true, "free").await;
    event(&f, 2, "'2026-10-25T01:00:00Z'", true, "free").await;
    event(&f, 3, "'2026-10-25T23:30:00Z'", false, "paid").await;
    let repo = PgAudience::new(f.app.clone());
    let report = repo.time_report(1, "Europe/Amsterdam").await.unwrap();
    totals_match(&report.current, (1, 2));
    let repeated: Vec<_> = report
        .current
        .hours
        .iter()
        .filter(|h| h.local_label == "2026-10-25 02:00")
        .collect();
    assert_eq!(repeated.len(), 2);
    assert_eq!(repeated[0].offset_minutes, 120);
    assert_eq!(repeated[1].offset_minutes, 60);
    assert_eq!(repeated[1].epoch - repeated[0].epoch, 3600);
    assert_eq!(repeated[0].counts, counts(0, 1));
    assert_eq!(repeated[1].counts, counts(0, 1));
    assert_eq!(report.current.by_hour[2].utc_hours, 2);
    assert_eq!(report.current.by_hour[2].counts, counts(0, 2));
    assert_eq!(report.current.by_weekday[0].counts, counts(1, 0));
    assert_eq!(report.current.by_weekday[6].counts, counts(0, 2));
    for (zone, local_hour) in [("UTC", 23), ("Europe/Moscow", 2), ("Asia/Yekaterinburg", 4)] {
        let zoned = repo.time_report(1, zone).await.unwrap();
        assert_eq!(zoned.until_epoch, report.until_epoch);
        assert_eq!(zoned.current.totals, report.current.totals);
        let row = zoned.current.hours.last().unwrap();
        assert_eq!(row.local_hour, local_hour);
        assert_eq!(row.weekday, if zone == "UTC" { 6 } else { 0 });
        assert_eq!(row.counts, counts(1, 0));
    }
    // Unknown in one repeated hour taints their shared category, not the other row.
    f.sql("DELETE FROM outbox_events WHERE aggregate_id=1")
        .await;
    let unknown = repo.time_report(1, "Europe/Amsterdam").await.unwrap();
    assert_eq!(unknown.current.hours[0].counts, None);
    assert_eq!(unknown.current.hours[1].counts, counts(0, 1));
    assert_eq!(unknown.current.by_hour[2].counts, None);
    assert_eq!(unknown.current.totals, None);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn readonly_permissions_locks_and_invalid_input_never_become_zero() {
    let f = fixture("2026-11-11T12:34:56Z").await;
    let repo = PgAudience::new(f.app.clone());
    for days in [0, 2, 91] {
        assert_eq!(
            repo.time_report(days, "UTC").await,
            Err(AudienceError::Invalid)
        );
    }
    for zone in ["", "Asia/Kathmandu", "Asia/Kolkata", "UTC;DROP TABLE users"] {
        assert_eq!(repo.time_report(7, zone).await, Err(AudienceError::Invalid));
    }
    for table in [
        "reader_chapter_starts",
        "reader_chapter_completions",
        "outbox_events",
    ] {
        let mut lock = f.owner.begin().await.unwrap();
        sqlx::query(&format!(
            "LOCK TABLE {}.{table} IN ACCESS EXCLUSIVE MODE",
            f.schema
        ))
        .execute(&mut *lock)
        .await
        .unwrap();
        let began = std::time::Instant::now();
        assert_eq!(
            repo.time_report(1, "UTC").await,
            Err(AudienceError::Unavailable)
        );
        assert!(began.elapsed() < std::time::Duration::from_secs(1));
        lock.rollback().await.unwrap();
    }
    f.sql("REVOKE SELECT ON reader_chapter_completions FROM tor_app")
        .await;
    assert_eq!(
        repo.time_report(1, "UTC").await,
        Err(AudienceError::Unavailable)
    );
    f.sql("GRANT SELECT ON reader_chapter_completions TO tor_app")
        .await;
    assert_eq!(
        repo.time_report(1, "UTC").await.unwrap().current.totals,
        counts(0, 0)
    );
    assert!(sqlx::query("DELETE FROM reader_chapter_starts")
        .execute(&f.app)
        .await
        .is_err());
    assert!(sqlx::query("DELETE FROM outbox_events")
        .execute(&f.app)
        .await
        .is_err());
    f.finish().await;
}
