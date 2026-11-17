//! RET-00b: an authenticated library must advance only after a trusted completion.
//! No purchase rights, archival grants or telemetry can be inferred from progress.
//! Run only on isolated PostgreSQL 18 tor_ci with applied migrations.
use sqlx::PgPool;
use tor_reading::{
    repositories::{PgReadingProgress, PgReadingSync},
    services::progress::ReadingPlace,
};

async fn chapter(
    pool: &PgPool,
    book: i64,
    number: i32,
    title: &str,
    visibility: &str,
    body: &str,
) -> (i64, i64) {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,$3,$4,'free',0) RETURNING id",
    )
    .bind(book)
    .bind(number)
    .bind(title)
    .bind(visibility)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    )
    .bind(id)
    .bind(body)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(id)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    (id, version)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn ret00_library_resume_moves_only_to_next_visible_free_chapter() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated database");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'RET00 Reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("ret00-reader-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'RET00 Stranger','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("ret00-other-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','RET00 book','public',$2,'basic') RETURNING id",
    )
    .bind(format!("ci:ret00:resume:{nonce}"))
    .bind(reader)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (first, first_version) = chapter(&pool, book, 1, "Первая", "public", "Первая глава").await;
    let (second, second_version) =
        chapter(&pool, book, 2, "Вторая", "public", "Вторая глава").await;
    let (third, _) = chapter(&pool, book, 3, "Черновик", "draft", "Третья глава").await;
    let _ = chapter(&pool, book, 4, "Четвёртая", "public", "Четвёртая глава").await;

    let progress = PgReadingProgress::new(pool.clone());
    let sync = PgReadingSync::new(pool.clone());
    assert!(progress.continue_reading(reader).await.unwrap().is_empty());
    assert!(progress.continue_reading(other).await.unwrap().is_empty());

    // Reading a portion of chapter 1 is not completion.
    let receipt = sync
        .save_free(
            reader,
            first,
            first_version,
            ReadingPlace {
                block_index: 1,
                char_offset: 2,
            },
            0,
        )
        .await
        .unwrap();
    assert_eq!(receipt.revision, 1);
    let partial = progress.continue_reading(reader).await.unwrap();
    assert_eq!(partial.len(), 1);
    assert_eq!(partial[0].chapter_id, first);
    assert_eq!(partial[0].block_index, 1);

    // A confirmed end-of-text position records a completion and advances the
    // displayed link, without moving the durable saved cursor.
    let finished_first = sync
        .save_free(
            reader,
            first,
            first_version,
            ReadingPlace {
                block_index: 1,
                char_offset: "Первая глава".encode_utf16().count() as i32,
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(finished_first.revision, 2);
    let next = progress.continue_reading(reader).await.unwrap();
    assert_eq!(next[0].chapter_id, second);
    assert_eq!(next[0].chapter_no, 2);
    assert_eq!(next[0].chapter_title, "Вторая");
    assert_eq!(next[0].block_index, 0);
    assert!(progress.continue_reading(other).await.unwrap().is_empty());
    let stored: i64 = sqlx::query_scalar(
        "SELECT chapter_id FROM reader_book_positions WHERE user_id=$1 AND book_id=$2",
    )
    .bind(reader)
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored, first,
        "reading card must not forge a saved position"
    );

    // Never skip a draft, paid, scheduled, held or deleted second chapter.
    sqlx::query("UPDATE chapters SET visibility='draft' WHERE id=$1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        progress.continue_reading(reader).await.unwrap()[0].chapter_id,
        first
    );
    sqlx::query(
        "UPDATE chapters SET visibility='public',access_level='paid',price_amount=100 WHERE id=$1",
    )
    .bind(second)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        progress.continue_reading(reader).await.unwrap()[0].chapter_id,
        first
    );
    sqlx::query(
        "UPDATE chapters SET access_level='free',price_amount=0,admin_hold=true WHERE id=$1",
    )
    .bind(second)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        progress.continue_reading(reader).await.unwrap()[0].chapter_id,
        first
    );
    sqlx::query("UPDATE chapters SET admin_hold=false,reader_released_at=now()+interval '1 hour' WHERE id=$1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        progress.continue_reading(reader).await.unwrap()[0].chapter_id,
        first
    );
    sqlx::query("UPDATE chapters SET reader_released_at=NULL WHERE id=$1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        progress.continue_reading(reader).await.unwrap()[0].chapter_id,
        second
    );

    // A new edition of chapter 1 invalidates the old anchor and must not
    // silently skip a rewritten chapter just because V1 was completed.
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Изменённая первая глава') RETURNING id",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(first)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    let changed = progress.continue_reading(reader).await.unwrap();
    assert_eq!(changed[0].chapter_id, first);
    assert_eq!(changed[0].block_index, 0);
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(first)
        .bind(first_version)
        .execute(&pool)
        .await
        .unwrap();

    // A revoked book age gate hides the entire resume path.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(progress.continue_reading(reader).await.unwrap().is_empty());
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    // Once chapter 2 is opened its real position, not the suggested chapter,
    // becomes authoritative. A completed chapter 2 must not skip draft #3
    // and jump to already-published chapter #4.
    let opened_second = sync
        .save_free(
            reader,
            second,
            second_version,
            ReadingPlace {
                block_index: 1,
                char_offset: 2,
            },
            2,
        )
        .await
        .unwrap();
    assert_eq!(opened_second.revision, 3);
    let resumed = progress.continue_reading(reader).await.unwrap();
    assert_eq!(resumed[0].chapter_id, second);
    assert_eq!(resumed[0].block_index, 1);
    sync.save_free(
        reader,
        second,
        second_version,
        ReadingPlace {
            block_index: 1,
            char_offset: "Вторая глава".encode_utf16().count() as i32,
        },
        3,
    )
    .await
    .unwrap();
    assert_eq!(
        progress.continue_reading(reader).await.unwrap()[0].chapter_id,
        second
    );

    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
        .bind(third)
        .execute(&pool)
        .await
        .unwrap();
    let new_next = progress.continue_reading(reader).await.unwrap();
    assert_eq!(new_next[0].chapter_id, third);
    assert_eq!(new_next[0].block_index, 0);
    pool.close().await;
}
