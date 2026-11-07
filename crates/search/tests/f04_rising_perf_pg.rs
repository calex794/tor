//! Large synthetic workload only: actual F-04 repository SQL with EXPLAIN.
//! C-verified provenance is tested separately via PgReadingSync in f04_rising_pg.
use std::time::{Duration, Instant};

use sqlx::PgPool;
use tor_search::{
    repositories::showcases::{PgShowcases, RISING_SHOWCASE_SQL},
    services::showcases::RisingShowcases,
};

#[tokio::test]
#[ignore = "only isolated F-02 100k PostgreSQL job, after f04_rising_seed.sql"]
async fn f_04_rising_plan_on_100k_books_and_100k_completions() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let pool = PgPool::connect(&url).await.expect("connect PostgreSQL");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci", "never profile on retained/live data");
    let (book_rows, completions): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM books),
                (SELECT COUNT(*) FROM reader_chapter_completions
                 WHERE user_id BETWEEN 9000001 AND 9005000)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(book_rows >= 100_000, "must profile large catalogue");
    assert_eq!(completions, 100_000, "must profile real-scale completion table");

    // Explain the literal production SQL, not a hand-maintained approximation.
    let plan_sql = format!("EXPLAIN (ANALYZE, BUFFERS) {RISING_SHOWCASE_SQL}");
    let plan: Vec<String> = sqlx::query_scalar(&plan_sql)
        .bind(12_i64)
        .fetch_all(&pool)
        .await
        .expect("F04 exact SQL plan");
    println!(
        "F04_100K_EXPLAIN books={book_rows} completions={completions}\n{}",
        plan.join("\n")
    );
    assert!(plan.iter().any(|line| line.contains("Execution Time:")));
    let pg = PgShowcases::new(pool.clone());
    let shelf = RisingShowcases::new(&pg);
    let mut samples = Vec::with_capacity(20);
    for _ in 0..20 {
        let start = Instant::now();
        let selected = shelf.rising_free_readers(12).await.unwrap();
        samples.push(start.elapsed());
        assert_eq!(selected.len(), 12, "large book set must yield capped shelf");
    }
    samples.sort();
    let p50 = samples[9];
    let p95 = samples[18];
    let max = samples[19];
    println!(
        "F04_100K_LATENCY n=20 books={book_rows} completions={completions} p50_ms={} p95_ms={} max_ms={}",
        p50.as_millis(),
        p95.as_millis(),
        max.as_millis()
    );
    assert!(
        p95 < Duration::from_secs(2),
        "F04 unindexed/read-expensive query cannot load the home page"
    );
}
