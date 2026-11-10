use super::fixtures::*;
use tor_recommendations::services::audience::{DiscoverySource, DISCOVERY_PERIOD_DAYS};

#[tokio::test]
#[ignore = "requires the installed retirement migration on disposable tor_ci"]
async fn partial_channel_buckets_stay_unknown_only_in_intersecting_periods() {
    let f = Fixture::new("analytics_invalid_source_windows", 9_158_100_000, 0).await;
    // Historical source totals survive removal of the public catalogue book.
    // A second book keeps a positive partial sum after each lost contribution.
    f.seed(f.base, 0, 179).await;
    f.seed(f.base + 1, 0, 179).await;
    f.invalidate(f.base, "recommendations", 0).await;
    f.mark(f.base, "catalog", 7).await;
    f.invalidate(f.base, "unattributed", 179).await;
    f.invalidate(f.base, "search", -1).await;
    f.invalidate(f.base, "search", 180).await;
    f.invalidate(f.base, "recommendations", 0).await;
    let retained_today: i64 = sqlx::query_scalar(
        "SELECT sum(paired_clicks+unpaired_clicks)::bigint FROM rec_quality_daily
         WHERE bucket_date=(now() AT TIME ZONE 'UTC')::date",
    )
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(retained_today, 5);
    let marked_but_retained: i64 = sqlx::query_scalar(
        "SELECT clicks FROM organic_discovery_quality_daily
         WHERE book_id=$1 AND source='catalog' AND bucket_date=(now() AT TIME ZONE 'UTC')::date-7",
    )
    .bind(f.base)
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(marked_but_retained, 3);
    let before: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rec_quality_daily),
                (SELECT count(*) FROM organic_discovery_quality_daily),
                (SELECT count(*) FROM rec_analytics_invalidations)",
    )
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(before.2, 5);
    let read_only: String = sqlx::query_scalar("SHOW default_transaction_read_only")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(read_only, "on");

    let sources = [
        DiscoverySource::Catalog,
        DiscoverySource::Search,
        DiscoverySource::Recommendations,
        DiscoverySource::Unattributed,
    ];
    let offsets: [&[i32]; 4] = [&[7], &[-1, 180], &[0], &[179]];
    let daily = [
        [Some(3), Some(5), Some(7), Some(11)],
        [Some(2), Some(3), Some(5), Some(7)],
        [Some(5), Some(7), Some(11), Some(13)],
        [None, None, Some(17), Some(19)],
    ];
    for days in DISCOVERY_PERIOD_DAYS {
        let report = f.store.report(days).await.unwrap();
        for (index, period) in [&report.current, &report.previous].into_iter().enumerate() {
            let from_offset = index as i32 * days;
            let until_offset = from_offset + days;
            for channel in 0..4 {
                let invalid = offsets[channel]
                    .iter()
                    .any(|day| *day >= from_offset && *day < until_offset);
                let value = &period.channels[channel];
                assert_eq!(value.source, sources[channel]);
                assert_eq!(value.evidence_invalidated, invalid);
                let expected = if invalid {
                    [None; 4]
                } else {
                    daily[channel].map(|v| v.map(|n| n * i64::from(days) * 2))
                };
                assert_eq!(channel_values(value), expected, "days={days}, {value:?}");
            }
        }
    }
    let books = f.store.books_report(&query(90, None, None)).await.unwrap();
    assert_eq!(books.matched_books, 0);
    assert_eq!(count_sum(&books.current.counts), 0);
    assert!(!books.current.totals.evidence_invalidated);
    let after: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rec_quality_daily),
                (SELECT count(*) FROM organic_discovery_quality_daily),
                (SELECT count(*) FROM rec_analytics_invalidations)",
    )
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(after, before, "read reports cannot rewrite retained evidence");
    f.finish().await;
}
