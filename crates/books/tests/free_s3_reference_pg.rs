//! S349: free S3 reference only binds exact B edition and is immutable.
//! This is deliberately NOT publication and NOT an entitlement test.
use sha2::{Digest, Sha256};
use sqlx::PgPool;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn s349_free_s3_reference_is_exact_immutable_and_not_a_paid_version() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated tor_ci");
    assert!(url.ends_with("/tor_ci"), "never modify production");
    let pool = PgPool::connect(&url).await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s349-free-owner@ci.invalid','S349 owner','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:s349:free','ru','S349 free','draft',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let free: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Free','draft','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'Paid','draft','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let markdown = "Старая редакция 😀";
    let hash: [u8; 32] = Sha256::digest(markdown.as_bytes()).into();
    let free_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(free)
    .bind(markdown)
    .bind(hash.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(paid)
    .bind("PAID-CONTENT")
    .bind(Sha256::digest(b"PAID-CONTENT").as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();

    let key = format!("private/free-chapters/{free}/{free_version}.md");
    let insert = "INSERT INTO free_chapter_storage_objects
        (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
        VALUES ($1,$2,'tor-private-chapter-ci',$3,$4,$5)";

    // A forged hash cannot bind a version to a different S3 blob.
    assert!(sqlx::query(insert)
        .bind(free)
        .bind(free_version)
        .bind(&key)
        .bind([7_u8; 32].as_slice())
        .bind(markdown.len() as i64)
        .execute(&pool)
        .await
        .is_err());
    // The key must be canonical for the exact version.
    assert!(sqlx::query(insert)
        .bind(free)
        .bind(free_version)
        .bind("private/free-chapters/1/1.md")
        .bind(hash.as_slice())
        .bind(markdown.len() as i64)
        .execute(&pool)
        .await
        .is_err());
    assert!(sqlx::query(insert)
        .bind(paid)
        .bind(paid_version)
        .bind(format!("private/free-chapters/{paid}/{paid_version}.md"))
        .bind(Sha256::digest(b"PAID-CONTENT").as_slice())
        .bind(12_i64)
        .execute(&pool)
        .await
        .is_err());
    sqlx::query(insert)
        .bind(free)
        .bind(free_version)
        .bind(&key)
        .bind(hash.as_slice())
        .bind(markdown.len() as i64)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sqlx::query(
        "UPDATE free_chapter_storage_objects SET object_key=$3
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(free)
    .bind(free_version)
    .bind("private/free-chapters/0/0.md")
    .execute(&pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "DELETE FROM free_chapter_storage_objects
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(free)
    .bind(free_version)
    .execute(&pool)
    .await
    .is_err());

    // Merely attaching a reference must never publish or grant reading.
    let publications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_publications WHERE chapter_id=$1")
            .bind(free)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(publications, 0);
    pool.close().await;
}
