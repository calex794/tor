use super::fixtures::*;
use tor_recommendations::services::audience::books::BookDiscoveryObservation;

#[tokio::test]
#[ignore = "requires the installed retirement migration on disposable tor_ci"]
async fn book_metrics_preserve_independent_channels_and_unknown_is_not_zero() {
    let f = Fixture::new("analytics_invalid_book_metrics", 9_158_200_000, 10).await;
    for offset in 0..5 {
        f.seed(f.base + offset, 0, 1).await;
    }
    f.invalidate(f.base, "recommendations", 0).await;
    f.invalidate(f.base + 1, "catalog", 0).await;
    f.mark(f.base + 2, "search", 0).await;
    f.invalidate(f.base + 3, "unattributed", 0).await;
    f.mark(f.base + 5, "recommendations", 0).await;
    // Book 6 has no observation; book 7 has measured zero in every channel.
    f.seed(f.base + 7, 0, 0).await;
    sqlx::query(
        "UPDATE rec_quality_daily SET impressions=0,paired_clicks=0,unpaired_clicks=0,
         book_opens=0,starts=0,completions=0 WHERE book_id=$1",
    )
    .bind(f.base + 7)
    .execute(&f.owner)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE organic_discovery_quality_daily SET issued=0,clicks=0,book_opens=0,
         starts=0,completions=0 WHERE book_id=$1",
    )
    .bind(f.base + 7)
    .execute(&f.owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO organic_discovery_quality_daily
         (bucket_date,book_id,source,issued,clicks,book_opens,starts,completions)
         VALUES((now() AT TIME ZONE 'UTC')::date,$1,'catalog',1,0,0,0,0),
               ((now() AT TIME ZONE 'UTC')::date,$2,'unattributed',0,0,0,1,0)",
    )
    .bind(f.base + 8)
    .bind(f.base + 9)
    .execute(&f.owner)
    .await
    .unwrap();

    let report = f.store.books_report(&query(1, None, None)).await.unwrap();
    assert_eq!(report.matched_books, 10);
    assert!(report.next_after_id.is_none());
    let expected = [
        [Some(30), None, None, None, None],
        [None, Some(100), None, None, None],
        [None, Some(100), None, None, None],
        [Some(30), Some(100), Some(10), Some(15), None],
        BOOK_VALUES.map(Some),
        [None; 5],
        [None; 5],
        [Some(0); 5],
        [Some(1), None, Some(0), Some(0), Some(0)],
        [None, None, None, None, Some(1)],
    ];
    let states = [
        BookDiscoveryObservation::UnknownEvidence,
        BookDiscoveryObservation::UnknownEvidence,
        BookDiscoveryObservation::UnknownEvidence,
        BookDiscoveryObservation::UnknownEvidence,
        BookDiscoveryObservation::WithStarts,
        BookDiscoveryObservation::UnknownEvidence,
        BookDiscoveryObservation::NoObservations,
        BookDiscoveryObservation::NoObservations,
        BookDiscoveryObservation::ObservedWithoutStarts,
        BookDiscoveryObservation::WithStarts,
    ];
    for (index, row) in report.rows.iter().enumerate() {
        assert_eq!(row.book_id, f.base + index as i64);
        assert_eq!(values(&row.current), expected[index]);
        assert_eq!(row.current.observation(), states[index]);
        assert_eq!(row.current.evidence_invalidated, index < 4 || index == 5);
        assert!(!row.previous.evidence_invalidated);
        assert_eq!(
            values(&row.previous),
            if index < 5 { BOOK_VALUES.map(Some) } else { [None; 5] }
        );
    }
    let counts = &report.current.counts;
    assert_eq!(
        (counts.unknown_evidence, counts.no_observations,
         counts.observed_without_starts, counts.with_starts),
        (5, 2, 1, 2)
    );
    assert_eq!(count_sum(counts), report.matched_books);
    assert_eq!(values(&report.current.totals), [None; 5]);
    assert!(report.current.totals.evidence_invalidated);
    assert_eq!(values(&report.previous.totals), BOOK_VALUES.map(|n| Some(n * 5)));
    assert_eq!(report.previous.counts.unknown_evidence, 0);
    assert_eq!(count_sum(&report.previous.counts), 10);

    let untouched = f.store.books_report(&query(1, Some("g-004"), None)).await.unwrap();
    assert_eq!(untouched.matched_books, 1);
    assert_eq!(values(&untouched.current.totals), BOOK_VALUES.map(Some));
    assert!(!untouched.current.totals.evidence_invalidated);
    let unattributed = f.store.books_report(&query(1, Some("g-003"), None)).await.unwrap();
    assert_eq!(
        values(&unattributed.current.totals),
        [Some(30), Some(100), Some(10), Some(15), None]
    );
    assert_eq!(unattributed.current.counts.unknown_evidence, 1);
    f.finish().await;
}
