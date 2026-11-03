//! S058 paid book-level cadence: prepared ciphertext is not a publication.
//! Database tests run only against disposable tor_ci PostgreSQL.
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgPool};
use tor_books::{
    repositories::{
        paid_storage::{PaidPublication, PgPaidStorage},
        postgres::{
            book_delayed_schedule::{BookSlotFrontier, BookSlotReleaseOutcome},
            PgAuthoring,
        },
    },
    services::authoring::{save_chapter, SaveChapterRequest},
};
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId, PositiveId};

async fn test_pool() -> (PgPool, sqlx::PgConnection) {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never touch production");
    let mut lock = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(580058)")
        .execute(&mut lock)
        .await
        .unwrap();
    (PgPool::connect(&url).await.unwrap(), lock)
}

async fn ready_object(
    pool: &PgPool,
    chapter: i64,
    version: i64,
    body: &str,
    raw_id: &str,
) -> ObjectId {
    let id = ObjectId::parse(raw_id).unwrap();
    let hash: [u8; 32] = Sha256::digest(body.as_bytes()).into();
    let aad = format!(
        "tor.encrypted.chapter.v1\n{chapter}\n{version}\n{}\n{}\n",
        id.as_str(),
        hex::encode(hash),
    );
    sqlx::query(
        "INSERT INTO storage_objects
          (id,object_kind,bucket,object_key,state,size_bytes,
           sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
           kek_id,object_nonce,object_aad,codec,verified_at,delete_after)
         VALUES($1::uuid,'chapter','tor-private-chapter-ci',$2,'ready',64,
                $3,$4,$5,$6,'content-kek-v1',$7,$8,'gzip',
                now(),now()+interval '24 hours')",
    )
    .bind(id.as_str())
    .bind(format!("private/chapter-versions/{}.enc", id.as_str()))
    .bind([4u8; 32].as_slice())
    .bind(hash.as_slice())
    .bind([8u8; 48].as_slice())
    .bind([3u8; 12].as_slice())
    .bind([2u8; 12].as_slice())
    .bind(aad.as_bytes())
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_prepared_paid_is_private_then_one_exact_slot_release() {
    let (pool, _lock) = test_pool().await;
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-paid-book-slot@example.invalid','S058 paid','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Scheduled paid chapter','public',$2) RETURNING id",
    )
    .bind(format!("ci:s058-paid:{nonce}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
         VALUES($1,1,'Paid hidden chapter','draft','paid',175,1) RETURNING id",
    ).bind(book).fetch_one(&pool).await.unwrap();
    let body = "Скрытая платная версия";
    let hash: [u8; 32] = Sha256::digest(body.as_bytes()).into();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(body)
    .bind(hash.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid = PgPaidStorage::new(pool.clone());
    let object = ready_object(
        &pool,
        chapter,
        version,
        body,
        "58005800-0001-4000-8000-000000000001",
    )
    .await;
    let pinned = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(version).unwrap(),
    };
    paid.attach_verified_and_publish(owner, book, pinned, &object, PaidPublication::KeepCurrent)
        .await
        .unwrap();
    assert_eq!(
        paid.staged_paid_object_attached(owner, book, chapter, version)
            .await,
        Ok(true)
    );
    let prior: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(prior, 0, "attached ciphertext cannot appear in reader TOC");
    let author = PgAuthoring::new(pool.clone());
    assert_eq!(
        paid.require_active_book_schedule(owner, book)
            .await
            .is_err(),
        true
    );
    author
        .replace_book_delayed_schedule(owner, book, true, 1, &["09:00".to_owned()])
        .await
        .unwrap();
    paid.require_active_book_schedule(owner, book)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE book_delayed_publication_settings
         SET next_slot_at=clock_timestamp()-interval '1 second'
         WHERE book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let token = "u".repeat(43);
    let claim = author.claim_due_book_slot(&token).await.unwrap().unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&claim, &token).await,
        Ok(BookSlotFrontier::Ready(chapter))
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM chapter_publications WHERE chapter_id=$1"
        )
        .bind(chapter)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
    );
    assert_eq!(
        author.release_bound_book_slot(&claim, &token).await,
        Ok(BookSlotReleaseOutcome::Released {
            chapter_id: chapter,
            version_id: version
        })
    );
    assert!(author
        .release_bound_book_slot(&claim, &token)
        .await
        .is_err());
    let row: (String, String, i64, i64) = sqlx::query_as(
        "SELECT c.visibility,c.access_level,c.price_amount,p.version_id
         FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
         WHERE c.id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("public".to_owned(), "paid".to_owned(), 175, version));
    let count: (i64, i64) = sqlx::query_as(
        "SELECT
         (SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1),
         (SELECT count(*) FROM chapter_encrypted_release_events WHERE chapter_id=$1)",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, (1, 1));
    let outbox: (String, String, i64) = sqlx::query_as(
        "SELECT event_type,dedupe_key,aggregate_id
         FROM outbox_events WHERE dedupe_key=$1",
    )
    .bind(format!("chapter-published:{chapter}:{version}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(outbox.0, "ChapterPublished");
    assert_eq!(outbox.1, format!("chapter-published:{chapter}:{version}"));
    assert_eq!(outbox.2, chapter);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_unprepared_paid_never_advances_past_frontier() {
    let (pool, _lock) = test_pool().await;
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-paid-block@example.invalid','S058 block','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Blocked paid slot','public',$2) RETURNING id",
    )
    .bind(format!("ci:s058-paid-block:{nonce}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter:i64=sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
         VALUES($1,1,'Unprepared paid chapter','draft','paid',210,1) RETURNING id",
    ).bind(book).fetch_one(&pool).await.unwrap();
    let author = PgAuthoring::new(pool.clone());
    author
        .replace_book_delayed_schedule(owner, book, true, 1, &["09:00".to_owned()])
        .await
        .unwrap();
    sqlx::query(
        "UPDATE book_delayed_publication_settings SET next_slot_at=clock_timestamp()-interval '1 second'
         WHERE book_id=$1",
    ).bind(book).execute(&pool).await.unwrap();
    let token = "v".repeat(43);
    let claim = author.claim_due_book_slot(&token).await.unwrap().unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&claim, &token).await,
        Ok(BookSlotFrontier::Blocked {
            chapter_id: Some(chapter),
            reason: "paid_version_not_prepared"
        })
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM chapter_publications WHERE chapter_id=$1"
        )
        .bind(chapter)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
    );
    pool.close().await;
}
