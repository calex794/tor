//! S058: book-slot claims must be durable, exclusive, and revision-fenced.
//! Writes are allowed only against disposable tor_ci.
use sqlx::{Connection, PgPool};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, AuthorError},
};

async fn disable_abandoned_due_slots(pool: &PgPool) {
    // Interrupted tests can leave an enabled, overdue schedule behind. The
    // production worker has a GLOBAL queue: a later test must not accidentally
    // claim that orphan and mistake it for its own newly created Book.
    // Never consume occurrences or touch other tests' or user schedules.
    let disabled: Vec<i64> = sqlx::query_scalar(
        r#"UPDATE book_delayed_publication_settings s
           SET enabled=FALSE,
               anchor_date=NULL,
               next_slot_at=NULL,
               next_slot_index=NULL,
               schedule_revision=s.schedule_revision+1,
               updated_at=clock_timestamp()
           FROM books b
           WHERE b.id=s.book_id AND s.enabled
             AND s.next_slot_at<=clock_timestamp()
             AND b.source_identity LIKE ANY (ARRAY[
                 'tor-ci:s058-release:%',
                 'tor-ci:s058-frontier:%',
                 'tor-ci:s058-occurrence:%',
                 'tor-ci:s058-queue-stress:%',
                 'ci:s058-paid:%',
                 'ci:s058-paid-block:%'
             ])
           RETURNING s.book_id"#,
    )
    .fetch_all(pool)
    .await
    .expect("clean disposable S058 test fixtures");
    if !disabled.is_empty() {
        eprintln!("S058 cleaned abandoned due test books: {disabled:?}");
    }
}

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
    disable_abandoned_due_slots(&pool).await;
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

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_global_queue_skips_locked_book_then_claims_it_after_rollback() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never touch production");
    // Coordinate global queue tests across binaries; this is not the Book
    // lock under test, which is held by a separate transaction below.
    let mut fixture_lock = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(580058)")
        .execute(&mut fixture_lock)
        .await
        .unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    disable_abandoned_due_slots(&pool).await;

    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-queue-stress@tor-ci.example','S058 queue','$argon2id$test')
         ON CONFLICT(email_normalized)
         DO UPDATE SET display_name=EXCLUDED.display_name RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let author = PgAuthoring::new(pool.clone());
    let make_book = |suffix: &str, key: u128| {
        new_book(
            &format!("tor-ci:s058-queue-stress:{suffix}:{nonce}"),
            "ru",
            &format!("S058 worker queue {suffix} {nonce}"),
            &format!("{key:043x}"),
        )
        .unwrap()
    };
    let oldest = author
        .create_owned_book(owner, &make_book("oldest", nonce))
        .await
        .unwrap();
    let newer = author
        .create_owned_book(owner, &make_book("newer", nonce + 1))
        .await
        .unwrap();
    for book in [oldest, newer] {
        author
            .replace_book_delayed_schedule(owner, book, true, 1, &slots())
            .await
            .unwrap();
    }
    // A historical fixed deadline makes both test books due before the
    // ordinary 'now - 1 second' fixtures; the ordering is deterministic.
    for (book, timestamp) in [
        (oldest, "2001-01-01 00:00:00+00"),
        (newer, "2001-01-02 00:00:00+00"),
    ] {
        sqlx::query(
            "UPDATE book_delayed_publication_settings
             SET next_slot_at=$2::timestamptz WHERE book_id=$1",
        )
        .bind(book)
        .bind(timestamp)
        .execute(&pool)
        .await
        .unwrap();
    }

    let mut hold = pool.begin().await.unwrap();
    let locked: i64 = sqlx::query_scalar("SELECT id FROM books WHERE id=$1 FOR UPDATE")
        .bind(oldest)
        .fetch_one(&mut *hold)
        .await
        .unwrap();
    assert_eq!(locked, oldest);
    let second_token = "s".repeat(43);
    let second = author
        .claim_due_book_slot(&second_token)
        .await
        .unwrap()
        .expect("SKIP LOCKED must claim the other due book");
    assert_eq!(
        second.book_id, newer,
        "unrelated due fixture or incorrect SKIP LOCKED order: {second:?}"
    );
    assert_eq!(second.attempt_count, 1);
    assert!(author
        .book_slot_claim_is_current(&second, &second_token)
        .await
        .unwrap());
    // Releasing the first Book lock must not lose its pending slot.
    hold.rollback().await.unwrap();
    let first_token = "t".repeat(43);
    let first = author
        .claim_due_book_slot(&first_token)
        .await
        .unwrap()
        .expect("the first book must be claimable immediately after unlock");
    assert_eq!(
        first.book_id, oldest,
        "the previously locked Book was skipped after rollback: {first:?}"
    );
    assert_eq!(first.attempt_count, 1);
    assert!(author
        .book_slot_claim_is_current(&first, &first_token)
        .await
        .unwrap());
    let (occurrences, published): (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM book_delayed_publication_occurrences
            WHERE book_id IN ($1,$2)),
           (SELECT count(*) FROM chapter_first_release_events e
            JOIN chapters c ON c.id=e.chapter_id
            WHERE c.book_id IN ($1,$2))",
    )
    .bind(oldest)
    .bind(newer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(occurrences, 2, "one durable occurrence per Book");
    assert_eq!(published, 0, "a slot claim is not a chapter publication");
    for book in [oldest, newer] {
        author
            .replace_book_delayed_schedule(owner, book, false, 1, &slots())
            .await
            .unwrap();
    }
    assert!(!author
        .book_slot_claim_is_current(&first, &first_token)
        .await
        .unwrap());
    assert!(!author
        .book_slot_claim_is_current(&second, &second_token)
        .await
        .unwrap());
    pool.close().await;
}
