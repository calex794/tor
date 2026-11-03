//! S020/#486: Shadow and REC must not expose age-restricted or deleting Books.
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_shadow_and_rec_recheck_live_age_tier_and_lifecycle() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable DB URL");
    assert!(
        url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"),
        "never modify persistent databases"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "tor_ci");
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let mut book_ids = Vec::<i64>::new();
    let mut chapter_ids = Vec::<i64>::new();
    for (suffix, tier) in [
        ("basic", Some("basic")),
        ("adult", Some("adult")),
        ("pro", Some("pro_adult")),
        ("unknown", None),
    ] {
        let book: i64 = sqlx::query_scalar(
            "INSERT INTO books (
                source_identity,target_language,title,visibility,visibility_status,
                access_level,shadow_enabled
             ) VALUES ($1,'ru',$2,'public','active',$3,TRUE) RETURNING id",
        )
        .bind(format!("ci:s020:shadow:{nonce}:{suffix}"))
        .bind(format!("S020 shadow {nonce} {suffix}"))
        .bind(tier)
        .fetch_one(&pool)
        .await
        .unwrap();
        let chapter: i64 = sqlx::query_scalar(
            "INSERT INTO chapters (
                book_id,chapter_no,title,visibility,access_level,price_amount
             ) VALUES ($1,1,$2,'public','free',0) RETURNING id",
        )
        .bind(book)
        .bind(format!("S020 chapter {suffix}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES($1,1,'PRIVATE-TEXT-NOT-IN-SHADOW') RETURNING id",
        )
        .bind(chapter)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)",
        )
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
        book_ids.push(book);
        chapter_ids.push(chapter);
    }
    let basic = book_ids[0];
    let count_books: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM shadow_public_books WHERE id=ANY($1::bigint[])",
    )
    .bind(&book_ids)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count_books, 1, "adult, pro_adult and NULL must be absent");
    let count_chapters: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM shadow_public_chapters WHERE id=ANY($1::bigint[])",
    )
    .bind(&chapter_ids)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count_chapters, 1, "chapter metadata must follow Book age");
    let rec_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rec_public_books WHERE id=ANY($1::bigint[])",
    )
    .bind(&book_ids)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rec_count, 1);

    // A live age change must revoke both public views without cached metadata.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(basic)
        .execute(&pool)
        .await
        .unwrap();
    let after_age: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM shadow_public_books WHERE id=ANY($1::bigint[])",
    )
    .bind(&book_ids)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_age, 0);
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(basic)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(basic)
        .execute(&pool)
        .await
        .unwrap();
    let after_lifecycle: (i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT COUNT(*) FROM shadow_public_books WHERE id=ANY($1::bigint[])),
            (SELECT COUNT(*) FROM shadow_public_chapters WHERE id=ANY($2::bigint[])),
            (SELECT COUNT(*) FROM rec_public_books WHERE id=ANY($1::bigint[]))",
    )
    .bind(&book_ids)
    .bind(&chapter_ids)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_lifecycle, (0, 0, 0), "pending deletion is not public");
}
