//! NEwBOO book-statistics facts must survive retry and never leak role data.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use tor_reading::repositories::book_stats::PgBookStats;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci migrated through SQLx"]
async fn b_stats_pg_book_uniqueness_repeat_visibility_and_purchases() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "no production SQL");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Book stats reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("book-stats-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','История <stats>','public') RETURNING id",
    )
    .bind(format!("ci:bookstats:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut chapter_ids = Vec::new();
    for number in 1..=10_i32 {
        let chapter: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES($1,$2,'Глава','public','free',0) RETURNING id",
        )
        .bind(book)
        .bind(number)
        .fetch_one(&pool)
        .await
        .unwrap();
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES($1,1,'test') RETURNING id",
        )
        .bind(chapter)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(chapter)
            .bind(version)
            .execute(&pool)
            .await
            .unwrap();
        chapter_ids.push(chapter);
    }
    let repo = PgBookStats::new(pool.clone());
    for chapter in &chapter_ids {
        repo.record_open(*chapter, Some(user)).await.unwrap();
    }
    repo.record_open(chapter_ids[0], Some(user)).await.unwrap();
    repo.record_open(chapter_ids[0], None).await.unwrap();
    let report = repo.report(book, None).await.unwrap().unwrap();
    assert_eq!(report.opens, 12);
    assert_eq!(report.unique_registered, 1);
    assert_eq!(report.anonymous_opens, 1);
    assert_eq!(report.repeat_authenticated, 1);
    assert_eq!(report.chapters.len(), 10);
    assert!(report.chapters.iter().all(|c| c.unique_registered == 1));
    assert_eq!(report.chapters[0].opens, 3);
    assert_eq!(report.chapters[0].repeat_authenticated, 1);
    assert_eq!(report.top_by_opens.first().map(|row| row.count), Some(3));
    assert!(report.top_by_purchases.is_empty());
    let next = repo.report(book, Some((5, chapter_ids[4]))).await.unwrap().unwrap();
    assert_eq!(next.chapters.len(), 5);
    assert!(next.chapters.iter().all(|chapter| chapter.number > 5));
    // Book holding must suppress further telemetry; history stays private for
    // moderation but the public reader must never start another open.
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    repo.record_open(chapter_ids[0], Some(user)).await.unwrap();
    assert!(repo.report(book, None).await.unwrap().is_none());
    // CASCADE deletes the per-reader identity when user is removed.
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM book_open_readers WHERE book_id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    // Transactional counters have no resurrected reader identity.
    pool.close().await;
}
