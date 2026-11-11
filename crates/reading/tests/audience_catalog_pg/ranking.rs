use super::fixture::Fixture;
use tor_reading::{
    repositories::audience::PgAudience,
    services::audience::{
        canonical::{CanonicalDimension as Dim, CanonicalQuery as Query},
        AudienceError,
    },
};

async fn populated() -> Fixture {
    let f = Fixture::new().await;
    f.books(43).await;
    for book in [41, 42, 43] {
        let count = if book == 43 { 9 } else { 10 };
        for user in 1..=count {
            f.completion(
                user,
                book,
                book * 100 + user,
                "now()-interval '2 hours'",
                "free",
            )
            .await;
        }
    }
    for user in 101..=110 {
        f.completion(user, 42, 42000 + user, "now()-interval '8 days'", "paid")
            .await;
    }
    f.completion(1, 1, 1, "now()-interval '2 hours'", "free")
        .await;
    // This established reader's later chapter must not add a ranked pair.
    f.completion(500, 41, 50000, "now()-interval '200 days'", "free")
        .await;
    f.completion(500, 41, 50001, "now()-interval '2 hours'", "free")
        .await;
    f.completion(600, 41, 60000, "now()-interval '2 hours'", "mismatch")
        .await;
    f
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn ranking_uses_full_population_with_deterministic_ties_and_small_sample_flags() {
    let f = populated().await;
    let store = PgAudience::new(f.app.clone());
    let query = Query::new(7, Dim::Books);
    let page = store.canonical_report(&query).await.unwrap();
    assert_eq!(page.rows.len(), 20);
    assert_eq!(page.rows.last().unwrap().book_id, Some(20));
    assert_eq!(page.next_after_id, Some(20));
    let rank = store.canonical_rank_report(&query).await.unwrap();
    assert_eq!(rank.rows.len(), 20);
    assert_eq!(rank.totals, page.totals);
    assert_eq!(rank.totals.books, 43);
    assert_eq!(rank.totals.current_pairs, 30);
    assert_eq!(rank.totals.previous_pairs, 10);
    assert_eq!(rank.rows[0].book_id, Some(41));
    assert_eq!(rank.rows[1].book_id, Some(42));
    assert_eq!(rank.rows[2].book_id, Some(43));
    assert_eq!(rank.rows[3].book_id, Some(1));
    assert_eq!(rank.rows[0].counts.current_pairs, 10);
    assert!(!rank.rows[0].small_current_sample);
    assert!(rank.rows[0].small_previous_sample);
    assert!(!rank.rows[1].small_previous_sample);
    assert!(rank.rows[2].small_current_sample);
    assert_eq!(rank.rows[2].counts.comparison_delta(), None);
    // A previous-window count never breaks a current-window tie.
    assert_eq!(
        store.canonical_rank_report(&query).await.unwrap().rows,
        rank.rows
    );
    let at = chrono::DateTime::parse_from_rfc3339(&rank.measured_at_utc).unwrap();
    let current = chrono::DateTime::parse_from_rfc3339(&rank.current_from_utc).unwrap();
    let previous = chrono::DateTime::parse_from_rfc3339(&rank.previous_from_utc).unwrap();
    assert_eq!((at - current).num_seconds(), 7 * 86400);
    assert_eq!((current - previous).num_seconds(), 7 * 86400);
    let mut cursor = query.clone();
    cursor.after_id = Some(20);
    assert_eq!(
        store.canonical_rank_report(&cursor).await,
        Err(AudienceError::Invalid)
    );
    assert_eq!(
        store.canonical_report(&cursor).await.unwrap().rows[0].book_id,
        Some(21)
    );
    // The app role is neither the schema owner nor a C writer in this fixture.
    let role: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(role, "tor_app");
    assert!(sqlx::query("DELETE FROM reader_chapter_completions")
        .execute(&f.app)
        .await
        .is_err());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn group_ranks_deduplicate_taxonomy_and_keep_filtered_totals_before_expansion() {
    let f = populated().await;
    f.sql("UPDATE reader_return_public_books SET genres=ARRAY['g-'||lpad(id::text,3,'0')],tags=ARRAY['t-'||lpad(id::text,3,'0')];
        UPDATE reader_return_public_books SET genres=ARRAY['z','z','shared'],tags=ARRAY['x','x'] WHERE id=41;
        UPDATE reader_return_public_books SET genres=ARRAY['y','shared'],tags=ARRAY['x'] WHERE id=42;
        UPDATE reader_return_public_books SET genres=ARRAY[]::text[],tags=ARRAY[]::text[] WHERE id=43").await;
    let store = PgAudience::new(f.app.clone());
    let rank = store
        .canonical_rank_report(&Query::new(7, Dim::Genres))
        .await
        .unwrap();
    assert_eq!(rank.rows.len(), 20);
    assert_eq!(rank.rows[0].label.as_deref(), Some("shared"));
    assert_eq!(rank.rows[0].counts.current_pairs, 20);
    assert_eq!(rank.rows[1].label.as_deref(), Some("y"));
    assert_eq!(rank.rows[2].label.as_deref(), Some("z"));
    assert_eq!(rank.rows[2].counts.current_pairs, 10);
    assert_eq!(rank.rows[3].label, None);
    assert_eq!(rank.rows[3].counts.current_pairs, 9);
    assert_eq!(rank.totals.current_pairs, 30);
    assert!(
        rank.rows
            .iter()
            .map(|row| row.counts.current_pairs)
            .sum::<i64>()
            > rank.totals.current_pairs
    );
    let tags = store
        .canonical_rank_report(&Query::new(7, Dim::Tags))
        .await
        .unwrap();
    assert_eq!(tags.rows[0].label.as_deref(), Some("x"));
    assert_eq!(tags.rows[0].counts.current_pairs, 20);
    let mut filtered = Query::new(7, Dim::Books);
    filtered.genre = Some(" SHARED ".into());
    filtered.tag = Some("x".into());
    let scoped = store.canonical_rank_report(&filtered).await.unwrap();
    assert_eq!(scoped.totals.books, 2);
    assert_eq!(scoped.totals.current_pairs, 20);
    assert_eq!(
        scoped
            .rows
            .iter()
            .map(|row| row.book_id.unwrap())
            .collect::<Vec<_>>(),
        [41, 42]
    );
    let mut missing = Query::new(7, Dim::Genres);
    missing.without_genre = true;
    let empty_label = store.canonical_rank_report(&missing).await.unwrap();
    assert_eq!(empty_label.totals.books, 1);
    assert_eq!(empty_label.rows[0].label, None);
    let mut cursor = Query::new(7, Dim::Tags);
    cursor.after_label = Some("x".into());
    assert_eq!(
        store.canonical_rank_report(&cursor).await,
        Err(AudienceError::Invalid)
    );
    // Removing a book from the approved current public port removes its rank.
    f.sql("DELETE FROM reader_return_public_books WHERE id=41")
        .await;
    let hidden = store
        .canonical_rank_report(&Query::new(7, Dim::Books))
        .await
        .unwrap();
    assert_eq!(hidden.totals.current_pairs, 20);
    assert!(hidden.rows.iter().all(|row| row.book_id != Some(41)));
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn ranking_locked_sources_are_unavailable_and_recover_without_mutation() {
    let f = Fixture::new().await;
    f.books(1).await;
    let store = PgAudience::new(f.app.clone());
    for table in [
        "reader_return_public_books",
        "reader_chapter_completions",
        "outbox_events",
        "reader_library_items",
        "reader_book_follows",
    ] {
        let mut tx = f.owner.begin().await.unwrap();
        sqlx::query(&format!(
            "LOCK TABLE {}.{table} IN ACCESS EXCLUSIVE MODE",
            f.schema
        ))
        .execute(&mut *tx)
        .await
        .unwrap();
        let began = std::time::Instant::now();
        assert_eq!(
            store
                .canonical_rank_report(&Query::new(7, Dim::Books))
                .await,
            Err(AudienceError::Unavailable)
        );
        assert!(began.elapsed() < std::time::Duration::from_secs(1));
        tx.rollback().await.unwrap();
    }
    assert_eq!(
        store
            .canonical_rank_report(&Query::new(7, Dim::Books))
            .await
            .unwrap()
            .totals
            .books,
        1
    );
    f.finish().await;
}
