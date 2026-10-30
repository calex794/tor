//! NEwBOO chapter thanks: real PostgreSQL idempotency and visibility.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use tor_reading::repositories::{PgChapterThanks, ThanksMutation};

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn c_thanks_pg_one_per_reader_chapter_concurrent_and_hidden() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "no production SQL");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Thanks reader','$argon2id$test') RETURNING id",
    )
    .bind(format!("thanks-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','Book thanks','public') RETURNING id",
    )
    .bind(format!("ci:thanks:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Глава 1','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Опубликованная бесплатная глава') RETURNING id",
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
    let repo = PgChapterThanks::new(pool.clone());
    let before = repo
        .visible_free_summary(user, chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.count, 0);
    assert!(!before.already_thanked);
    let (first, second) = tokio::join!(
        repo.thank_visible_free(user, chapter),
        repo.thank_visible_free(user, chapter)
    );
    let results = [first.unwrap(), second.unwrap()];
    assert!(results.contains(&ThanksMutation::Added));
    assert!(results.contains(&ThanksMutation::Repeated));
    assert_eq!(
        repo.thank_visible_free(user, chapter).await.unwrap(),
        ThanksMutation::Repeated
    );
    let after = repo
        .visible_free_summary(user, chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.count, 1);
    assert!(after.already_thanked);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chapter_thanks WHERE book_id=$1 AND chapter_id=$2",
    )
    .bind(book)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);

    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .visible_free_summary(user, chapter)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.thank_visible_free(user, chapter).await.unwrap(),
        ThanksMutation::NotVisible
    );

    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter_thanks WHERE chapter_id=$1")
        .bind(chapter)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "right-to-delete cascades reader identity");
    pool.close().await;
}
