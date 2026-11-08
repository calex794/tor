//! S058: book-slot claims must be durable, exclusive, and revision-fenced.
//! Writes are allowed only against disposable tor_ci.
use sqlx::{Connection, PgPool};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, AuthorError},
};

fn slots() -> Vec<String> {
    vec!["09:00".to_owned(), "18:00".to_owned()]
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_due_slot_is_claimed_once_and_reclaimed_only_after_lease() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never run on production");
    // A database-scoped fixture lock coordinates separate Cargo test binaries.
    // Dropping this connection releases the lock, including on panic.
    let mut slot_fixture_lock = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(580058)")
        .execute(&mut slot_fixture_lock)
        .await
        .unwrap();
    let pool = PgPool::connect(&url).await.expect("disposable postgres");
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-occurrence-owner@tor-ci.example','S058 Owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let author = PgAuthoring::new(pool.clone());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title = format!("Очередь публикаций {nonce}");
    let key = format!("{nonce:043x}");
    let book = new_book(
        &format!("tor-ci:s058-occurrence:{owner}:{nonce}"),
        "ru",
        &title,
        &key,
    )
    .unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    let settings = author
        .replace_book_delayed_schedule(owner, book_id, true, 1, &slots())
        .await
        .unwrap();
    assert_eq!(settings.schedule_revision, 1);
    let a = "a".repeat(43);
    let b = "b".repeat(43);
    let c = "c".repeat(43);
    assert_eq!(
        author.claim_due_book_slot("bad").await,
        Err(AuthorError::Invalid)
    );
    assert!(author.claim_due_book_slot(&a).await.unwrap().is_none());

    sqlx::query(
        "UPDATE book_delayed_publication_settings
         SET next_slot_at=clock_timestamp()-interval '1 second' WHERE book_id=$1",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    let (left, right) = tokio::join!(
        author.claim_due_book_slot(&a),
        author.claim_due_book_slot(&b),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left.is_some(), right.is_some(), "only one worker may claim");
    let (claim, token, loser) = if let Some(claim) = left {
        (claim, &a, &b)
    } else {
        (right.unwrap(), &b, &a)
    };
    assert_eq!(claim.book_id, book_id);
    assert_eq!(claim.schedule_revision, 1);
    assert_eq!(claim.attempt_count, 1);
    assert!(author
        .book_slot_claim_is_current(&claim, token)
        .await
        .unwrap());
    assert!(!author
        .book_slot_claim_is_current(&claim, loser)
        .await
        .unwrap());
    assert!(author.claim_due_book_slot(loser).await.unwrap().is_none());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM book_delayed_publication_occurrences WHERE book_id=$1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);

    sqlx::query(
        "UPDATE book_delayed_publication_occurrences
         SET processing_lease_expires_at=clock_timestamp()-interval '1 second'
         WHERE book_id=$1",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!author
        .book_slot_claim_is_current(&claim, token)
        .await
        .unwrap());
    let retry = author.claim_due_book_slot(&c).await.unwrap().unwrap();
    assert_eq!(retry.book_id, claim.book_id);
    assert_eq!(retry.schedule_revision, claim.schedule_revision);
    assert_eq!(retry.local_date, claim.local_date);
    assert_eq!(retry.slot_index, claim.slot_index);
    assert_eq!(retry.target_utc, claim.target_utc);
    assert_eq!(retry.attempt_count, 2);
    assert!(!author
        .book_slot_claim_is_current(&retry, token)
        .await
        .unwrap());
    assert!(author.book_slot_claim_is_current(&retry, &c).await.unwrap());

    // Schedule edits invalidate the old occurrence without deleting evidence.
    let changed = author
        .replace_book_delayed_schedule(owner, book_id, true, 1, &["10:00".to_owned()])
        .await
        .unwrap();
    assert_eq!(changed.schedule_revision, 2);
    assert!(!author.book_slot_claim_is_current(&retry, &c).await.unwrap());
    assert!(author.claim_due_book_slot(&c).await.unwrap().is_none());
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_first_release_events e
                            JOIN chapters ch ON ch.id=e.chapter_id WHERE ch.book_id=$1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 0, "claiming a slot must not release any chapter");
    pool.close().await;
}
