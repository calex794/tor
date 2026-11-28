//! Real PostgreSQL preparation tests; no reader or S3 provider activation.
use super::{fixture, update};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::{
    repositories::{
        free_s3_publish::PgFreeS3Publication,
        guest_prefix::{GuestPrefixPreparation, GuestPrefixTarget, PgGuestPrefix},
    },
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};
use tor_contracts::guest_prefix::{GuestPrefix, GUEST_PREFIX_ALGORITHM};

async fn pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG CI URL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    PgPool::connect(&url).await.unwrap()
}

async fn prepare_book(pool: &PgPool, book: i64) {
    // Legacy fixtures have NULL age tier (B-09). Set the known public tier
    // explicitly for this isolated fixture; never change production defaults.
    // The SQL rollout fence remains closed.
    let changed = sqlx::query(
        "UPDATE books SET access_level='basic',guest_policy_revision=1,
                          guest_open_through_chapter_no=0
         WHERE id=$1 AND guest_read_mode='open' AND access_level IS NULL",
    )
    .bind(book)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(changed.rows_affected(), 1);
}

async fn current_prefix(pool: &PgPool, chapter: i64) -> Option<String> {
    // Exact current-pointer join used to prove stale rows cannot be selected.
    // This is a test query, not an authorized reader path.
    sqlx::query_scalar(
        "SELECT g.prefix_text FROM chapter_publications p
         JOIN chapter_version_guest_prefixes g
           ON g.chapter_id=p.chapter_id AND g.version_id=p.version_id
         WHERE p.chapter_id=$1 AND g.algorithm_version=$2",
    )
    .bind(chapter)
    .bind(GUEST_PREFIX_ALGORITHM)
    .fetch_optional(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci"]
async fn auth_read_prefix_fences_old_editions_policy_and_wrong_source() {
    let pool = pool().await;
    let (author, owner, _, book, original) = fixture(&pool, "guest-prefix-v1-v2").await;
    let repo = PgGuestPrefix::new(pool.clone());
    let first = GuestPrefixTarget {
        book_id: book,
        chapter_id: original.chapter_id,
        version_id: original.version_id,
        policy_revision: 1,
    };
    let source = "Оригинальный текст V1 😀";
    // A prepared policy alone cannot turn an unknown age tier into basic.
    let changed = sqlx::query(
        "UPDATE books SET guest_policy_revision=1 WHERE id=$1 AND access_level IS NULL",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    assert_eq!(
        repo.prepare_current(first, source).await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(current_prefix(&pool, first.chapter_id).await, None);
    prepare_book(&pool, book).await;
    assert_eq!(
        repo.prepare_current(first, "different source").await,
        Err(AuthorError::Conflict)
    );
    let retention_before: (i64, i64, i64) = sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions,physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(first.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.prepare_current(first, source).await,
        Ok(GuestPrefixPreparation::Stored)
    );
    let retention_after: (i64, i64, i64) = sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions,physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(first.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retention_before, retention_after);
    assert_eq!(
        repo.prepare_current(first, source).await,
        Ok(GuestPrefixPreparation::AlreadyStored)
    );
    assert_eq!(
        current_prefix(&pool, first.chapter_id).await.as_deref(),
        Some(GuestPrefix::from_source(source).unwrap().maximum_prefix())
    );

    let body = format!(
        "SECOND edition visible beginning. {} V2_TAIL",
        "слово ".repeat(100)
    );
    let command = update(
        book,
        first.chapter_id,
        original.revision,
        "Second edition",
        &body,
        &"p".repeat(43),
    );
    let edited = author
        .edit_published_free_chapter(owner, &command)
        .await
        .unwrap();
    assert_ne!(edited.version_id, first.version_id);
    assert_eq!(current_prefix(&pool, first.chapter_id).await, None);
    assert_eq!(
        repo.prepare_current(first, source).await,
        Err(AuthorError::Conflict)
    );
    let second = GuestPrefixTarget {
        version_id: edited.version_id,
        ..first
    };
    assert_eq!(
        repo.prepare_current(second, source).await,
        Err(AuthorError::Conflict)
    );
    let hash: [u8; 32] = Sha256::digest(body.as_bytes()).into();
    let wrong = sqlx::query(
        "INSERT INTO chapter_version_guest_prefixes
         (chapter_id,version_id,algorithm_version,source_sha256,
          visible_scalar_count,prefix_text,prefix_sha256)
         VALUES($1,$2,$3,$4,10,'x',sha256(convert_to('x','UTF8')))",
    )
    .bind(second.chapter_id)
    .bind(second.version_id)
    .bind(GUEST_PREFIX_ALGORITHM)
    .bind([0_u8; 32].as_slice())
    .execute(&pool)
    .await;
    assert!(wrong.is_err());
    let bad_digest = sqlx::query(
        "INSERT INTO chapter_version_guest_prefixes
         (chapter_id,version_id,algorithm_version,source_sha256,
          visible_scalar_count,prefix_text,prefix_sha256)
         VALUES($1,$2,$3,$4,10,'x',$5)",
    )
    .bind(second.chapter_id)
    .bind(second.version_id)
    .bind(GUEST_PREFIX_ALGORITHM)
    .bind(hash.as_slice())
    .bind([0_u8; 32].as_slice())
    .execute(&pool)
    .await;
    assert!(bad_digest.is_err());
    assert_eq!(
        repo.prepare_current(second, &body).await,
        Ok(GuestPrefixPreparation::Stored)
    );
    let current = current_prefix(&pool, second.chapter_id).await.unwrap();
    assert!(current.starts_with("SECOND edition"));
    assert!(!current.contains("V2_TAIL"));
    assert!(!current.contains("Оригинальный"));
    assert!(current.chars().count() <= 2000 && current.len() <= 8000);
    let versions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_version_guest_prefixes WHERE chapter_id=$1",
    )
    .bind(second.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(versions, 2);
    assert!(sqlx::query(
        "UPDATE chapter_version_guest_prefixes SET prefix_text='' WHERE version_id=$1",
    )
    .bind(second.version_id)
    .execute(&pool)
    .await
    .is_err());

    sqlx::query(
        "UPDATE books SET guest_policy_revision=2,guest_open_through_chapter_no=7 WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.prepare_current(second, &body).await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        repo.prepare_current(
            GuestPrefixTarget {
                policy_revision: 2,
                ..second
            },
            &body,
        )
        .await,
        Err(AuthorError::Conflict)
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci"]
async fn auth_read_prefix_handles_bodyless_edition_without_pg_full_text() {
    let pool = pool().await;
    let (_, owner, _, book, _) = fixture(&pool, "guest-prefix-bodyless").await;
    prepare_book(&pool, book).await;
    let source = format!("Bodyless beginning. {} PRIVATE_TAIL", "长文 ".repeat(300));
    let request = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 8,
        expected_revision: 0,
        title: "Bodyless guest prefix",
        body: &source,
        access: "free",
        price_amount: 0,
        command_key: &"q".repeat(43),
    })
    .unwrap();
    let publisher = PgFreeS3Publication::new(pool.clone());
    let edition = publisher.prepare_new(owner, &request).await.unwrap();
    let hash: [u8; 32] = Sha256::digest(request.body.as_bytes()).into();
    // Metadata-only fixture, not evidence of real MinIO/S3 retrieval.
    publisher
        .attach_verified_and_publish(
            owner,
            book,
            (edition.chapter_id, edition.version_id),
            ("tor-private-auth-read-ci", hash, request.body.len() as u64),
        )
        .await
        .unwrap();
    let target = GuestPrefixTarget {
        book_id: book,
        chapter_id: edition.chapter_id,
        version_id: edition.version_id,
        policy_revision: 1,
    };
    let repo = PgGuestPrefix::new(pool.clone());
    assert_eq!(
        repo.prepare_current(target, "tampered object").await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        repo.prepare_current(target, &request.body).await,
        Ok(GuestPrefixPreparation::Stored)
    );
    let stored: (bool, Vec<u8>, Vec<u8>, String) = sqlx::query_as(
        "SELECT v.body_markdown IS NULL,g.source_sha256,g.prefix_sha256,g.prefix_text
         FROM chapter_versions v JOIN chapter_version_guest_prefixes g
           ON g.chapter_id=v.chapter_id AND g.version_id=v.id
         WHERE v.id=$1 AND g.algorithm_version=$2",
    )
    .bind(edition.version_id)
    .bind(GUEST_PREFIX_ALGORITHM)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(stored.0, "bodyless S3 edition remains bodyless");
    assert_eq!(stored.1.as_slice(), hash.as_slice());
    let prefix_hash: [u8; 32] = Sha256::digest(stored.3.as_bytes()).into();
    assert_eq!(stored.2.as_slice(), prefix_hash.as_slice());
    assert!(stored.3.starts_with("Bodyless beginning."));
    assert!(!stored.3.contains("PRIVATE_TAIL"));
}
