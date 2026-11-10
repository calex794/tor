use super::fixtures::*;
use std::collections::BTreeSet;
use tor_recommendations::services::audience::groups::{
    GroupDiscoveryKind, GroupDiscoveryQuery, GroupDiscoveryReport, GroupDiscoveryRow,
};

async fn all_groups(
    f: &Fixture,
    kind: GroupDiscoveryKind,
) -> (GroupDiscoveryReport, Vec<GroupDiscoveryRow>) {
    let first = f.store.groups_report(&GroupDiscoveryQuery::new(1, kind, None).unwrap())
        .await.unwrap();
    let mut rows = first.rows.clone();
    let mut cursor = first.next_after.clone();
    let mut pages = 1;
    while let Some(after) = cursor {
        assert!(pages < 4, "bounded fixture pagination must make progress");
        let page = f.store.groups_report(&GroupDiscoveryQuery::new(1, kind, Some(after)).unwrap())
            .await.unwrap();
        assert_eq!(page.current, first.current);
        assert_eq!(page.previous, first.previous);
        assert_eq!(page.matched_groups, first.matched_groups);
        assert_eq!(page.matched_books, first.matched_books);
        rows.extend(page.rows);
        cursor = page.next_after;
        pages += 1;
    }
    assert!(pages > 1, "this fixture must exercise later pages");
    assert_eq!(rows.len() as i64, first.matched_groups);
    let labels: BTreeSet<_> = rows.iter().map(|row| row.label.clone()).collect();
    assert_eq!(labels.len(), rows.len(), "duplicate labels cannot multiply groups");
    (first, rows)
}

#[tokio::test]
#[ignore = "requires the installed retirement migration on disposable tor_ci"]
async fn invalidation_precedes_paging_and_overlapping_groups_keep_unique_book_counts() {
    let f = Fixture::new("analytics_invalid_group_pages", 9_158_300_000, 27).await;
    for offset in 0..27 {
        f.seed(f.base + offset, 0, 0).await;
    }
    sqlx::query(
        "UPDATE books SET genres=ARRAY['all','all','g-025','overlap','overlap'],
         tags=ARRAY['tag-all','tag-all','t-025','tag-overlap','tag-overlap'] WHERE id=$1",
    )
    .bind(f.base + 25)
    .execute(&f.owner)
    .await
    .unwrap();
    sqlx::query("UPDATE books SET genres=ARRAY[]::text[],tags=ARRAY[]::text[] WHERE id=$1")
        .bind(f.base + 26)
        .execute(&f.owner)
        .await
        .unwrap();
    f.invalidate(f.base + 24, "recommendations", 0).await;
    f.invalidate(f.base + 25, "catalog", 0).await;
    f.invalidate(f.base + 25, "catalog", 0).await;

    let first = f.store.books_report(&query(1, None, None)).await.unwrap();
    assert_eq!(first.rows.len(), 20);
    assert!(first.rows.iter().all(|row| !row.current.evidence_invalidated));
    assert_eq!(first.current.counts.unknown_evidence, 2);
    assert_eq!(first.current.counts.with_starts, 25);
    assert_eq!(count_sum(&first.current.counts), 27);
    assert_eq!(values(&first.current.totals), [None; 5]);
    let second = f.store.books_report(&query(1, None, first.next_after_id))
        .await.unwrap();
    assert_eq!(second.rows.len(), 7);
    assert!(second.next_after_id.is_none());
    assert_eq!(second.current, first.current);
    assert_eq!(second.previous, first.previous);
    assert_eq!(second.rows.iter().filter(|row| row.current.evidence_invalidated).count(), 2);

    for kind in [GroupDiscoveryKind::Genres, GroupDiscoveryKind::Tags] {
        let (report, rows) = all_groups(&f, kind).await;
        assert_eq!(report.matched_books, 27);
        assert_eq!(report.matched_groups, 29);
        assert_eq!(report.current.counts.unknown_evidence, 2);
        assert_eq!(count_sum(&report.current.counts), 27);
        assert_eq!(report.previous.counts.unknown_evidence, 0);
        let common = if kind == GroupDiscoveryKind::Genres { "all" } else { "tag-all" };
        let overlap = if kind == GroupDiscoveryKind::Genres { "overlap" } else { "tag-overlap" };
        let common_row = rows.iter().find(|row| row.label.as_deref() == Some(common)).unwrap();
        assert_eq!(common_row.matched_books, 26);
        assert_eq!(common_row.current_counts.unknown_evidence, 2);
        assert_eq!(common_row.current_counts.with_starts, 24);
        assert_eq!(values(&common_row.current), [None; 5]);
        let overlap_row = rows.iter().find(|row| row.label.as_deref() == Some(overlap)).unwrap();
        assert_eq!(overlap_row.matched_books, 1);
        assert_eq!(overlap_row.current_counts.unknown_evidence, 1);
        assert_eq!(values(&overlap_row.current), [None, Some(100), None, None, None]);
        let missing = rows.iter().find(|row| row.label.is_none()).unwrap();
        assert_eq!(missing.matched_books, 1);
        assert_eq!(values(&missing.current), BOOK_VALUES.map(Some));
        assert_eq!(missing.current_counts.unknown_evidence, 0);
        for row in &rows {
            assert_eq!(count_sum(&row.current_counts), row.matched_books);
        }
        let summed_unknown: i64 = rows.iter().map(|row| row.current_counts.unknown_evidence).sum();
        assert!(summed_unknown > report.current.counts.unknown_evidence,
            "overlapping group counts cannot replace unique-book totals");
    }
    let overlap = f.store.books_report(&query(1, Some("overlap"), None)).await.unwrap();
    assert_eq!(overlap.matched_books, 1);
    assert_eq!(values(&overlap.current.totals), [None, Some(100), None, None, None]);
    assert_eq!(overlap.current.counts.unknown_evidence, 1);

    // Current eligibility filters precede both masks and totals, not pagination.
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(f.base + 24)
        .execute(&f.owner)
        .await
        .unwrap();
    let hidden = f.store.books_report(&query(1, None, None)).await.unwrap();
    assert_eq!(hidden.matched_books, 26);
    assert_eq!(hidden.current.counts.unknown_evidence, 1);
    assert_eq!(hidden.current.totals.impressions, Some(2600));
    for kind in [GroupDiscoveryKind::Genres, GroupDiscoveryKind::Tags] {
        let groups = f.store.groups_report(&GroupDiscoveryQuery::new(1, kind, None).unwrap())
            .await.unwrap();
        assert_eq!(groups.matched_books, 26);
        assert_eq!(groups.current, hidden.current);
    }
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(f.base + 24)
        .execute(&f.owner)
        .await
        .unwrap();
    let source = f.store.report(1).await.unwrap();
    assert!(source.current.channels[2].evidence_invalidated);
    assert_eq!(channel_values(&source.current.channels[2]), [None; 4]);
    let retained_marker: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rec_analytics_invalidations
         WHERE book_id=$1 AND source='recommendations'",
    )
    .bind(f.base + 24)
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(retained_marker, 1, "global history cannot silently become complete after deletion");
    f.finish().await;
}
