//! D-001: explicit historical anchors preserve exact bytes without minting
//! paid access. This is a real PostgreSQL 18 schema test, never a GC test.
use sha2::{Digest, Sha256};
use sqlx::PgPool;

#[tokio::test]
#[ignore = "disposable PostgreSQL 18 tor_ci only"]
async fn d001_comments_and_text_issues_preserve_exact_historical_editions() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG18");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"));
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let pool = PgPool::connect(&url).await.unwrap();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('d001-anchors-owner@example.invalid','D001','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:d001:historical-anchors','ru','D001 anchors','public',$1)
         RETURNING id",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Anchored chapter','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();

    let mut versions = Vec::new();
    for (number, text) in [(1, "Old V1"), (2, "Old V2"), (3, "Current V3")] {
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
             VALUES($1,$2,$3,$4) RETURNING id",
        )
        .bind(chapter)
        .bind(number)
        .bind(text)
        .bind(digest.as_slice())
        .fetch_one(&pool)
        .await
        .unwrap();
        versions.push(version);
    }
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(versions[2])
        .execute(&pool)
        .await
        .unwrap();

    let before: (i64, i64, i64) = sqlx::query_as(
        "SELECT logical_editions, reader_retained_editions, physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, (1, 1, 3));
    let unpinned_before: i64 = sqlx::query_scalar(
        "SELECT superseded_unpinned_materializations
         FROM d001_registered_overhang_by_chapter WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unpinned_before, 2);

    sqlx::query(
        "INSERT INTO discussion_comments
         (author_user_id,chapter_id,version_id,body,visibility,command_key,command_hash)
         VALUES($1,$2,$3,'Specific V1 comment','visible',$4,$5)",
    )
    .bind(actor)
    .bind(chapter)
    .bind(versions[0])
    .bind("a".repeat(43))
    .bind([4_u8; 32].as_slice())
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO text_issues
         (book_id,chapter_id,version_id,reporter_user_id,block_index,
          selection_start,selection_end,quote,suggested_text,issue_type,request_hash)
         VALUES($1,$2,$3,$4,1,0,3,'Old','Fix','typo',$5)",
    )
    .bind(book)
    .bind(chapter)
    .bind(versions[1])
    .bind(actor)
    .bind([5_u8; 32].as_slice())
    .execute(&pool)
    .await
    .unwrap();

    let after: (i64, i64, i64) = sqlx::query_as(
        "SELECT logical_editions, reader_retained_editions, physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        after,
        (3, 1, 3),
        "anchors preserve bytes, not reader licenses"
    );
    let unpinned_after: i64 = sqlx::query_scalar(
        "SELECT superseded_unpinned_materializations
         FROM d001_registered_overhang_by_chapter WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unpinned_after, 0);

    let flags: (bool, bool) = sqlx::query_as(
        "SELECT has_discussion_anchor,has_text_issue_anchor
         FROM d001_chapter_materialization_roles
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(versions[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(flags, (true, false));

    let rejected =
        sqlx::query("UPDATE chapter_versions SET body_markdown='Unsafe rewrite' WHERE id=$1")
            .bind(versions[0])
            .execute(&pool)
            .await;
    assert!(rejected.is_err(), "retained text still immutable");

    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
         VALUES($1,$2,$3)",
    )
    .bind(actor)
    .bind(chapter)
    .bind(versions[0])
    .execute(&pool)
    .await
    .unwrap();
    let saved: (i64, i64) = sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved, (3, 2));
    pool.close().await;
}
