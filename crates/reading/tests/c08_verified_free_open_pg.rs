//! C-08 / D-001: Book bookmarks only retain exact already-opened FREE text.
//! All database writes run in disposable PG18 tor_ci under GitHub Actions.
use sqlx::PgPool;
use tor_reading::repositories::{free_open_capture::PgFreeOpenCapture, PgLibrary};

async fn count(pool: &PgPool, actor: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM reader_saved_free_chapters WHERE user_id=$1")
        .bind(actor)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn saved_version(pool: &PgPool, actor: i64, chapter: i64) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT version_id FROM reader_saved_free_chapters
         WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(actor)
    .bind(chapter)
    .fetch_optional(pool)
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PG18 tor_ci + GitHub Actions only"]
async fn c08_book_save_only_opened_exact_free_editions_and_races() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG18");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"));
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user1: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'C08 reader 1','$argon2id$test') RETURNING id",
    )
    .bind(format!("c08-opened-1-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let user2: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'C08 reader 2','$argon2id$test') RETURNING id",
    )
    .bind(format!("c08-opened-2-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','C08 only opened','public','basic') RETURNING id",
    )
    .bind(format!("ci:c08:open-only:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut chapters = Vec::new();
    for number in 1..=100_i32 {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES($1,$2,'free','public','free',0) RETURNING id",
        )
        .bind(book)
        .bind(number)
        .fetch_one(&pool)
        .await
        .unwrap();
        let edition: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES($1,1,'Exact first FREE text') RETURNING id",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES($1,$2)",
        )
        .bind(id)
        .bind(edition)
        .execute(&pool)
        .await
        .unwrap();
        chapters.push((id, edition));
    }
    let open = PgFreeOpenCapture::new(pool.clone());
    let library = PgLibrary::new(pool.clone());
    // Saving a Book without opening any chapter MUST NOT pin 100 editions.
    library.add_book(user1, book).await.unwrap();
    assert_eq!(count(&pool, user1).await, 0);
    let (chapter3, v3) = chapters[2];
    let (chapter8, v8) = chapters[7];
    assert!(open.record(user1, chapter3, v3).await.unwrap());
    assert_eq!(saved_version(&pool, user1, chapter3).await, Some(v3));
    assert_eq!(count(&pool, user1).await, 1);
    assert!(open.record(user1, chapter8, v8).await.unwrap());
    assert_eq!(count(&pool, user1).await, 2);
    assert!(!open.record(user1, chapter3, i64::MAX).await.unwrap());
    assert_eq!(count(&pool, user1).await, 2);

    // Pre-bookmark evidence must bind the exact *opened* version, even
    // after the author has automatically published a replacement.
    assert!(open.record(user2, chapter3, v3).await.unwrap());
    assert_eq!(count(&pool, user2).await, 0);
    let new3: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Exact replacement V2') RETURNING id",
    )
    .bind(chapter3)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter3)
        .bind(new3)
        .execute(&pool)
        .await
        .unwrap();
    library.add_book(user2, book).await.unwrap();
    assert_eq!(saved_version(&pool, user2, chapter3).await, Some(v3));
    assert_eq!(count(&pool, user2).await, 1);
    assert!(open.record(user2, chapter3, new3).await.unwrap());
    assert_eq!(saved_version(&pool, user2, chapter3).await, Some(v3));
    library.add_book(user2, book).await.unwrap();
    assert_eq!(count(&pool, user2).await, 1);
    assert_eq!(saved_version(&pool, user1, chapter3).await, Some(v3));

    // Concurrent bookmark and first-open cannot lose the opened FREE edition.
    let (chapter20, v20) = chapters[19];
    let added = {
        let lib = library.clone();
        tokio::spawn(async move { lib.add_book(user2, book).await })
    };
    let viewed = {
        let opener = open.clone();
        tokio::spawn(async move { opener.record(user2, chapter20, v20).await })
    };
    added.await.unwrap().unwrap();
    assert!(viewed.await.unwrap().unwrap());
    assert_eq!(saved_version(&pool, user2, chapter20).await, Some(v20));

    // Mere progress is not a historical chapter license. First-open
    // evidence itself is logically retained but not a paid entitlement.
    let (logical, reader, physical): (i64, i64, i64) = sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions,physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter3)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((logical, reader, physical), (2, 2, 2));
    // A signed source may be authorized at V1, then author V2 commits
    // while the browser still downloads and SHA-checks the original bytes.
    // This pre-existing actor/version/SHA intent must survive the switch.
    let (chapter30, v30) = chapters[29];
    let checked_sha = [0x3a_u8; 32];
    let checked_hex = "3a".repeat(32);
    open.issue_intent(user1, chapter30, v30, checked_sha)
        .await
        .unwrap();
    assert!(
        !open
            .record_confirmed(user1, chapter30, v30, &"ff".repeat(32))
            .await
            .unwrap(),
        "wrong SHA must never create historical FREE rights"
    );
    let new30: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Published concurrently with old S3 GET') RETURNING id",
    )
    .bind(chapter30)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter30)
        .bind(new30)
        .execute(&pool)
        .await
        .unwrap();
    assert!(open
        .record_confirmed(user1, chapter30, v30, &checked_hex)
        .await
        .unwrap());
    assert_eq!(saved_version(&pool, user1, chapter30).await, Some(v30));
    assert!(open
        .record_confirmed(user1, chapter30, v30, &checked_hex)
        .await
        .unwrap());
    assert_eq!(saved_version(&pool, user1, chapter30).await, Some(v30));

    // An expired permit never creates an open: an old version/sha tuple
    // alone must not be accepted as evidence of a reader delivery.
    let (chapter31, v31) = chapters[30];
    open.issue_intent(user1, chapter31, v31, checked_sha)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE reader_free_open_intents
         SET expires_at=now()-interval '1 second'
         WHERE user_id=$1 AND chapter_id=$2 AND version_id=$3",
    )
    .bind(user1)
    .bind(chapter31)
    .bind(v31)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!open
        .record_confirmed(user1, chapter31, v31, &checked_hex)
        .await
        .unwrap());
    assert_eq!(saved_version(&pool, user1, chapter31).await, None);
    pool.close().await;
}
