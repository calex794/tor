//! Bounded real model page with 100k metadata candidates and a shared three-slot pool.
use super::contextual::{command, metadata, stop};
use super::*;
use tor_recommendations::services::contextual::Action;
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci current model schema"]
async fn model_page_is_bounded_with_100k_books_and_reader_requests() {
    let f = fixture("contextual-scale").await;
    metadata(&f).await;
    let language = format!("ctx-{}", f.books[0]);
    // Earlier CI scenarios have explicit book IDs beyond the sequence. Reserve
    // a disjoint range under a table lock; do not reset a shared sequence or
    // ignore conflicts, which would silently shrink the measured workload.
    let mut setup = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *setup)
        .await
        .unwrap();
    // Reproduce the full-CI condition: an explicit ID is ahead of nextval.
    let sentinel: i64 = sqlx::query_scalar("INSERT INTO books(id,source_identity,target_language,title,visibility,access_level) SELECT coalesce(max(id),0)+93610,'ci:contextual-scale-sentinel:'||$1,'zz','Unrelated explicit ID','draft','basic' FROM books RETURNING id")
        .bind(&language).fetch_one(&mut *setup).await.unwrap();
    sqlx::query("INSERT INTO books(id,source_identity,target_language,title,visibility,access_level,genres,tags) SELECT (SELECT coalesce(max(id),0) FROM books)+n, 'ci:contextual-scale:'||$1||':'||n,$1,'Metadata scale '||n,'public','basic',ARRAY['shared'],ARRAY['system'] FROM generate_series(1,100000) n")
        .bind(&language).execute(&mut *setup).await.unwrap();
    setup.commit().await.unwrap();
    let candidates: i64 = sqlx::query_scalar("SELECT count(*) FROM books WHERE target_language=$1 AND source_identity LIKE 'ci:contextual-scale:%'")
        .bind(&language)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(candidates, 100000, "all scale candidates were inserted");
    let generation = command(&f, Action::Refresh, None).await;
    // Skip only pre-existing fixture books so this page contains the chosen
    // seed; no model is marked finished and all runtime SQL remains real.
    sqlx::query("UPDATE rec_generations SET cursor_book_id=$2 WHERE id=$1::uuid")
        .bind(&generation)
        .bind(f.books[0] - 1)
        .execute(&f.pool)
        .await
        .unwrap();
    let shared = PgPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_millis(500))
        .connect(&current_analytics::url("TEST_DATABASE_URL", "tor_app"))
        .await
        .unwrap();
    let store = PgRecommendations::new(shared.clone());
    let start = std::time::Instant::now();
    let (page, reader) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(2),
            store.advance_contextual_generation()
        ),
        tokio::time::timeout(
            Duration::from_secs(2),
            store.feed(None, Some(f.books[0]), 20)
        )
    );
    assert!(page.expect("whole background page deadline").unwrap());
    assert_catalog(&reader.expect("reader blocked by builder").unwrap());
    let cursor: i64 =
        sqlx::query_scalar("SELECT cursor_book_id FROM rec_generations WHERE id=$1::uuid")
            .bind(&generation)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(cursor, f.books[0], "one tick visits exactly one seed");
    let seeds: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT seed_id) FROM rec_candidate_edges WHERE generation_id=$1::uuid",
    )
    .bind(&generation)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(seeds, 1);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rec_candidate_edges WHERE generation_id=$1::uuid")
            .bind(&generation)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(rows <= 70, "bounded content partitions and manual edges");
    eprintln!(
        "CONTEXTUAL_100K seeds={seeds} edges={rows} elapsed_ms={}",
        start.elapsed().as_millis()
    );
    command(&f, Action::Cancel, Some(&generation)).await;
    shared.close().await;
    stop(&f, &[generation]).await;
    sqlx::query("DELETE FROM books WHERE target_language=$1 AND source_identity LIKE 'ci:contextual-scale:%'")
        .bind(&language)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM books WHERE id=$1 AND source_identity='ci:contextual-scale-sentinel:'||$2",
    )
    .bind(sentinel)
    .bind(&language)
    .execute(&f.pool)
    .await
    .unwrap();
    f.cleanup().await;
}
