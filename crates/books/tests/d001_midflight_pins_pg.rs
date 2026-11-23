//! D-001: an exact FREE save or Layer archive between prepare and attach
//! must retain the old immutable materialization after the author publishes.
//!
//! This is a deterministic two-phase interleaving test, not a proof that
//! concurrent buyer/editor transactions serialize. No S3 DELETE or GC here.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::{
    repositories::free_s3_publish::PgFreeS3Publication,
    services::authoring::{save_chapter, SaveChapter, SaveChapterRequest},
};

fn save(
    book_id: i64,
    chapter_id: Option<i64>,
    revision: i64,
    body: &str,
    key_char: char,
) -> SaveChapter {
    let key = key_char.to_string().repeat(43);
    save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: chapter_id.map(|id| id as u64),
        expected_revision: revision,
        number: 1,
        title: "D001 midflight",
        body,
        access: "free",
        price_amount: 0,
        command_key: &key,
    })
    .unwrap()
}

fn metadata(body: &str) -> (&'static str, [u8; 32], u64) {
    (
        "tor-private-s349-ci",
        Sha256::digest(body.as_bytes()).into(),
        body.len() as u64,
    )
}

async fn counts(pool: &PgPool, chapter_id: i64) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions,physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn current(pool: &PgPool, chapter_id: i64) -> i64 {
    sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
        .bind(chapter_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn verify_receipt(pool: &PgPool, chapter_id: i64, version_id: i64, body: &str) {
    let expected: [u8; 32] = Sha256::digest(body.as_bytes()).into();
    let (version_sha, receipt_sha): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT v.content_sha256,f.sha256
         FROM chapter_versions v
         JOIN free_chapter_storage_objects f
           ON f.chapter_id=v.chapter_id AND f.version_id=v.id
         WHERE v.chapter_id=$1 AND v.id=$2",
    )
    .bind(chapter_id)
    .bind(version_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(version_sha, expected);
    assert_eq!(receipt_sha, expected);
}

#[tokio::test]
#[ignore = "disposable PG18 tor_ci + GitHub CI only"]
async fn d001_saved_free_and_layer_pins_midflight_preserve_old_exact_versions() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG18");
    assert!(
        url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"),
        "D-001 may never modify persistent PostgreSQL"
    );
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let pool = PgPool::connect(&url).await.unwrap();
    let suffix = std::process::id();

    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D001 midflight owner','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("d001-midflight-owner-{suffix}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D001 midflight reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("d001-midflight-reader-{suffix}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','D001 midflight','public',$2) RETURNING id",
    )
    .bind(format!("ci:d001:midflight:{suffix}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let store = PgFreeS3Publication::new(pool.clone());

    let initial = save(book_id, None, 0, "Exact original V1 body", 'a');
    let v1 = store.prepare_new(owner, &initial).await.unwrap();
    store
        .attach_verified_and_publish(
            owner,
            book_id,
            (v1.chapter_id, v1.version_id),
            metadata(&initial.body),
        )
        .await
        .unwrap();
    let chapter = v1.chapter_id;
    assert_eq!(counts(&pool, chapter).await, (1, 1, 1));

    // The author reserves V2, but V1 is still the reader-visible head.
    // The reader must be able to save that exact V1 before the PG switch.
    let v2_input = save(book_id, Some(chapter), 1, "Different V2 body", 'b');
    let v2 = store.prepare_edit(owner, &v2_input).await.unwrap();
    assert!(!v2.published);
    assert_ne!(v2.version_id, v1.version_id);
    assert_eq!(current(&pool, chapter).await, v1.version_id);

    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
         VALUES($1,$2,$3)",
    )
    .bind(reader)
    .bind(chapter)
    .bind(v1.version_id)
    .execute(&pool)
    .await
    .unwrap();
    store
        .attach_edit(owner, &v2_input, &v2, metadata(&v2_input.body))
        .await
        .unwrap();
    assert_eq!(current(&pool, chapter).await, v2.version_id);
    assert_eq!(counts(&pool, chapter).await, (2, 2, 2));

    let v1_saved: bool = sqlx::query_scalar(
        "SELECT has_saved_free AND is_reader_retained
         FROM d001_chapter_materialization_roles WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v1.version_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(v1_saved);

    // The same two-phase window must preserve an exact Layer FREE archive.
    // Archiving is a retention obligation, not permission to mutate the bytes.
    let v3_input = save(book_id, Some(chapter), 2, "Different V3 body", 'c');
    let v3 = store.prepare_edit(owner, &v3_input).await.unwrap();
    assert!(!v3.published);
    assert_eq!(current(&pool, chapter).await, v2.version_id);
    sqlx::query(
        "INSERT INTO book_layer_free_archive_versions
         (book_id,request_revision,chapter_id,version_id) VALUES($1,1,$2,$3)",
    )
    .bind(book_id)
    .bind(chapter)
    .bind(v2.version_id)
    .execute(&pool)
    .await
    .unwrap();
    store
        .attach_edit(owner, &v3_input, &v3, metadata(&v3_input.body))
        .await
        .unwrap();
    assert_eq!(current(&pool, chapter).await, v3.version_id);
    assert_eq!(counts(&pool, chapter).await, (3, 3, 3));

    let v2_layer: bool = sqlx::query_scalar(
        "SELECT has_layer_free_archive AND is_reader_retained
         FROM d001_chapter_materialization_roles WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v2.version_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(v2_layer);
    verify_receipt(&pool, chapter, v1.version_id, &initial.body).await;
    verify_receipt(&pool, chapter, v2.version_id, &v2_input.body).await;
    verify_receipt(&pool, chapter, v3.version_id, &v3_input.body).await;

    // A successful retry cannot allocate another physical identity or rebind
    // a saved FREE or Layer pointer to the newest revision.
    let replay = store.prepare_edit(owner, &v3_input).await.unwrap();
    assert!(replay.published);
    assert_eq!(replay.version_id, v3.version_id);
    assert_eq!(counts(&pool, chapter).await, (3, 3, 3));
    let stale = save(book_id, Some(chapter), 2, "Stale V3 proposal", 'd');
    assert!(store.prepare_edit(owner, &stale).await.is_err());
    assert_eq!(counts(&pool, chapter).await, (3, 3, 3));
    pool.close().await;
}
