//! BR-060/C-08: 10k saved-free chapters use bounded, stable private cursors.
//! Run only against the disposable tor_ci PostgreSQL database in CI.
use sqlx::PgPool;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tor_reading::{repositories::PgLibrary, services::library::LibraryError};

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL pointing at loopback tor_ci"]
async fn br060_archive_10k_deep_keyset_is_bounded_and_preserves_exact_v1() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci");
    assert!(
        url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"),
        "never access permanent or remote PostgreSQL"
    );
    let pool = PgPool::connect(&url).await.expect("tor_app database");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Archive 10k','$argon2id$test') RETURNING id",
    )
    .bind(format!("br060-archive-10k-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut digest = [0_u8; 32];
    digest[..16].copy_from_slice(&nonce.to_le_bytes());
    digest[16..].copy_from_slice(&nonce.to_le_bytes());
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(actor)
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','BR060 10k archive','public','basic') RETURNING id",
    )
    .bind(format!("ci:br060:archive:10k:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    // Three set-based inserts, no per-chapter HTTP or round-trip overhead.
    sqlx::query(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         SELECT $1, n, 'Глава ' || n, 'public', 'free', 0
         FROM generate_series(1,10000) AS numbers(n)",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         SELECT id, 1, 'Pinned immutable V1 for archive test'
         FROM chapters WHERE book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id)
         SELECT c.id,v.id FROM chapters AS c
         JOIN chapter_versions AS v ON v.chapter_id=c.id AND v.version_no=1
         WHERE c.book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    let library = PgLibrary::new(pool.clone());
    library.add_book(actor, book).await.unwrap();
    let pins: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_saved_free_chapters AS f
         JOIN chapters AS c ON c.id=f.chapter_id
         WHERE f.user_id=$1 AND c.book_id=$2",
    )
    .bind(actor)
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pins, 10_000, "all released free V1 chapters were pinned");

    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         ordinary_deleted_at=now(), visibility='draft' WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    let started = Instant::now();
    let first = library
        .saved_archive_chapters(actor, book, 100, None, &digest)
        .await
        .unwrap();
    let first_ms = started.elapsed().as_millis();
    assert_eq!(first.items.len(), 100);
    assert_eq!(first.items[0].chapter_no, 1);
    assert_eq!(first.items[99].chapter_no, 100);
    assert_eq!(first.next_cursor, Some(100));
    assert!(first
        .items
        .iter()
        .all(|item| { item.saved_free_version_no == Some(1) && item.purchased.is_none() }));

    let started = Instant::now();
    let deep = library
        .saved_archive_chapters(actor, book, 100, Some(9900), &digest)
        .await
        .unwrap();
    let deep_ms = started.elapsed().as_millis();
    assert_eq!(deep.items.len(), 100);
    assert_eq!(deep.items[0].chapter_no, 9901);
    assert_eq!(deep.items[99].chapter_no, 10_000);
    assert_eq!(deep.next_cursor, None);
    assert!(deep
        .items
        .iter()
        .all(|item| { item.saved_free_version_no == Some(1) && item.purchased.is_none() }));

    let middle = library
        .saved_archive_chapters(actor, book, 30, Some(9950), &digest)
        .await
        .unwrap();
    assert_eq!(middle.items.len(), 30);
    assert_eq!(middle.items[0].chapter_no, 9951);
    assert_eq!(middle.items[29].chapter_no, 9980);
    assert_eq!(middle.next_cursor, Some(9980));
    let end = library
        .saved_archive_chapters(actor, book, 30, middle.next_cursor, &digest)
        .await
        .unwrap();
    assert_eq!(end.items.len(), 20);
    assert_eq!(end.items[0].chapter_no, 9981);
    assert_eq!(end.items[19].chapter_no, 10_000);
    assert_eq!(end.next_cursor, None);
    assert!(library
        .saved_archive_chapters(actor, book, 100, Some(10_000), &digest)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        library
            .saved_archive_chapters(actor, book, 101, None, &digest)
            .await
            .err(),
        Some(LibraryError::Invalid)
    );

    // Revoked sessions and hard/moderation holds cannot enumerate the archive.
    let denied_digest = [42_u8; 32];
    assert!(library
        .saved_archive_chapters(actor, book, 100, None, &denied_digest)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(library
        .saved_archive_chapters(actor, book, 100, None, &digest)
        .await
        .unwrap()
        .items
        .is_empty());

    // Timings are emitted for CI evidence; do not invent an environment-neutral
    // hard millisecond SLA from one disposable runner.
    eprintln!("BR-060 10k archive: first page {first_ms} ms, deep page {deep_ms} ms");
    pool.close().await;
}
