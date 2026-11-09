//! S349: additive free S3 receipt schema; no existing reader or author path changes.
#![cfg(feature = "pg")]
use sqlx::{PgPool, Row};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn s349_free_storage_reference_is_edition_exact_immutable_and_private() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG18 CI database");
    assert!(url.ends_with("/tor_ci"), "never touch a live database");
    let pool = PgPool::connect(&url).await.unwrap();

    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s349-owner@example.invalid','S349 owner','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:s349:metadata','ru','S349 private file','draft',$1)
         RETURNING id",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .unwrap();
    let free_chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Free chapter','draft','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'Paid chapter','draft','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let text = "Неизменяемая бесплатная глава";
    let digest: Vec<u8> = sqlx::query_scalar("SELECT sha256(convert_to($1,'UTF8'))")
        .bind(text)
        .fetch_one(&pool)
        .await
        .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(free_chapter)
    .bind(text)
    .bind(&digest)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,'paid',sha256(convert_to('paid','UTF8'))) RETURNING id",
    )
    .bind(paid_chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    let key = format!("private/free-chapters/{free_chapter}/{version}.md");
    let saved: i64 = sqlx::query_scalar(
        "INSERT INTO free_chapter_storage_objects
         (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
         VALUES($1,$2,'tor-private-free-ci',$3,$4,$5)
         RETURNING version_id",
    )
    .bind(free_chapter)
    .bind(version)
    .bind(&key)
    .bind(&digest)
    .bind(text.len() as i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved, version);
    let meta = sqlx::query(
        "SELECT object_key,content_type,sha256 FROM free_chapter_storage_objects
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(free_chapter)
    .bind(version)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(meta.get::<String, _>("object_key"), key);
    assert_eq!(
        meta.get::<String, _>("content_type"),
        "text/plain; charset=utf-8"
    );
    assert_eq!(meta.get::<Vec<u8>, _>("sha256"), digest);

    // Hash and chapter access are authoritative; a receipt cannot cross
    // from a paid version, or masquerade as an arbitrary S3 object.
    let forged_paid = sqlx::query(
        "INSERT INTO free_chapter_storage_objects
         (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
         VALUES($1,$2,'tor-private-free-ci',$3,$4,10)",
    )
    .bind(paid_chapter)
    .bind(paid_version)
    .bind(format!(
        "private/free-chapters/{paid_chapter}/{paid_version}.md"
    ))
    .bind(&digest)
    .execute(&pool)
    .await;
    assert!(forged_paid.is_err());
    let wrong_hash = sqlx::query(
        "INSERT INTO free_chapter_storage_objects
         (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
         VALUES($1,$2,'tor-private-free-ci',$3,$4,10)",
    )
    .bind(free_chapter)
    .bind(version)
    .bind(&key)
    .bind(vec![1_u8; 32])
    .execute(&pool)
    .await;
    assert!(wrong_hash.is_err());
    assert!(sqlx::query(
        "UPDATE free_chapter_storage_objects SET size_bytes=15
             WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(free_chapter)
    .bind(version)
    .execute(&pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "DELETE FROM free_chapter_storage_objects
             WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(free_chapter)
    .bind(version)
    .execute(&pool)
    .await
    .is_err());
    pool.close().await;
}
