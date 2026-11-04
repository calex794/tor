//! S058: B-06 chapter release and book schedule slot must commit together.
//! Every writing test runs only on disposable /tor_ci PostgreSQL.
use sqlx::{Connection, PgPool};
use tor_books::{
    repositories::postgres::{
        book_delayed_schedule::{BookSlotFrontier, BookSlotReleaseOutcome},
        PgAuthoring,
    },
    services::authoring::{new_book, save_chapter, AuthorError, SaveChapterRequest},
};

async fn fixture(pool: &PgPool, suffix: &str) -> (PgAuthoring, i64, i64) {
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Book slot owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .bind(format!("s058-release-{suffix}@tor-ci.example"))
    .fetch_one(pool)
    .await
    .unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let book = new_book(
        &format!("tor-ci:s058-release:{user}:{suffix}:{nonce}"),
        "ru",
        &format!("S058 release {suffix} {nonce}"),
        &format!("{nonce:043x}"),
    )
    .unwrap();
    let author = PgAuthoring::new(pool.clone());
    let book_id = author.create_owned_book(user, &book).await.unwrap();
    let first = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Уже доступная",
        body: "Первая глава",
        access: "free",
        price_amount: 0,
        command_key: &format!("{:043x}", ((book_id as u128) << 32) | 1),
    })
    .unwrap();
    author.create_free_chapter(user, &first).await.unwrap();
    author.release_book(user, book_id).await.unwrap();
    (author, user, book_id)
}

async fn pending(author: &PgAuthoring, owner: i64, book: i64, number: i32) -> (i64, i64) {
    let key = format!("{:043x}", ((book as u128) << 32) | (number as u128));
    let request = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number,
        expected_revision: 0,
        title: "Приватная бесплатная",
        body: "Текст закрытой главы",
        access: "free",
        price_amount: 0,
        command_key: &key,
    })
    .unwrap();
    let saved = author
        .create_pending_free_chapter(owner, &request)
        .await
        .unwrap();
    (saved.chapter_id, saved.version_id)
}

async fn due(pool: &PgPool, book: i64) {
    sqlx::query(
        "UPDATE book_delayed_publication_settings
         SET next_slot_at=clock_timestamp()-interval '1 second' WHERE book_id=$1",
    )
    .bind(book)
    .execute(pool)
    .await
    .unwrap();
}

async fn slot_test_pool() -> (PgPool, sqlx::PgConnection) {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never touch production");
    let mut lock = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(580058)")
        .execute(&mut lock)
        .await
        .unwrap();
    (PgPool::connect(&url).await.unwrap(), lock)
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_bound_slot_release_is_one_atomic_b06_version_and_cursor() {
    let (pool, _lock) = slot_test_pool().await;
    let (author, owner, book) = fixture(&pool, "atomic").await;
    let (chapter, _) = pending(&author, owner, book, 2).await;
    let times = ["09:00".to_owned(), "18:00".to_owned()];
    author
        .replace_book_delayed_schedule(owner, book, true, 1, &times)
        .await
        .unwrap();
    due(&pool, book).await;
    let token = "n".repeat(43);
    let claim = author.claim_due_book_slot(&token).await.unwrap().unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&claim, &token).await,
        Ok(BookSlotFrontier::Ready(chapter))
    );
    let before: Option<i64> =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(before, None, "slot binding cannot publish a chapter");

    // An edit after binding is the current immutable edition to release,
    // never a stale version from a separate prior read.
    let edited_request = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: Some(chapter as u64),
        number: 2,
        expected_revision: 1,
        title: "Переписана до выпуска",
        body: "Новая редакция перед публикацией",
        access: "free",
        price_amount: 0,
        command_key: &format!("{:043x}", ((book as u128) << 32) | 4),
    })
    .unwrap();
    let edited = author
        .edit_unpublished_free_chapter(owner, &edited_request)
        .await
        .unwrap();
    assert_eq!(edited.revision, 2);
    assert_eq!(
        author
            .release_bound_book_slot(&claim, &"p".repeat(43))
            .await,
        Err(AuthorError::Conflict),
        "another worker's token cannot release"
    );
    let result = author
        .release_bound_book_slot(&claim, &token)
        .await
        .unwrap();
    assert_eq!(
        result,
        BookSlotReleaseOutcome::Released {
            chapter_id: chapter,
            version_id: edited.version_id,
        }
    );
    assert_eq!(
        author.release_bound_book_slot(&claim, &token).await,
        Err(AuthorError::Conflict),
        "the completed occurrence cannot re-release on replay"
    );
    let persisted: (String, i64, i64, String) = sqlx::query_as(
        "SELECT c.visibility,p.version_id,f.version_id,v.body_markdown
         FROM chapters c
         JOIN chapter_publications p ON p.chapter_id=c.id
         JOIN chapter_first_release_events f ON f.chapter_id=c.id
         JOIN chapter_versions v ON v.id=p.version_id
         WHERE c.id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted.0, "public");
    assert_eq!(persisted.1, edited.version_id);
    assert_eq!(persisted.2, edited.version_id);
    assert_eq!(persisted.3, "Новая редакция перед публикацией");
    let event_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(event_count, 1);
    let draft_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_working_drafts WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(draft_count, 0);
    let occurrence: (String, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT status,processing_token,chapter_id
         FROM book_delayed_publication_occurrences WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(occurrence.0, "completed");
    assert_eq!(occurrence.1, None);
    assert_eq!(occurrence.2, Some(chapter));
    let next_index: i32 = sqlx::query_scalar(
        "SELECT next_slot_index FROM book_delayed_publication_settings WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    // The first due slot depends on Moscow local time. A claimed 09:00
    // slot advances to 18:00; a claimed 18:00 slot wraps to tomorrow's 09:00.
    // Assert the cursor transition, not the runner's wall clock.
    assert_eq!(
        next_index,
        (claim.slot_index + 1) % (times.len() as i32),
        "the next cursor must follow the claimed slot"
    );
    author
        .replace_book_delayed_schedule(owner, book, false, 1, &times)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_bound_slot_does_not_release_after_hold_or_stale_lease() {
    let (pool, _lock) = slot_test_pool().await;
    let (author, owner, book) = fixture(&pool, "held").await;
    let (chapter, _) = pending(&author, owner, book, 2).await;
    let times = ["09:00".to_owned()];
    author
        .replace_book_delayed_schedule(owner, book, true, 1, &times)
        .await
        .unwrap();
    due(&pool, book).await;
    let token = "q".repeat(43);
    let claim = author.claim_due_book_slot(&token).await.unwrap().unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&claim, &token).await,
        Ok(BookSlotFrontier::Ready(chapter))
    );
    sqlx::query("UPDATE chapters SET admin_hold=TRUE WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        author.release_bound_book_slot(&claim, &token).await,
        Ok(BookSlotReleaseOutcome::Blocked {
            chapter_id: chapter,
            reason: "chapter_admin_hold",
        })
    );
    let blocked: (String, String) = sqlx::query_as(
        "SELECT status,last_error_code FROM book_delayed_publication_occurrences
         WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        blocked,
        ("blocked".to_owned(), "chapter_admin_hold".to_owned())
    );
    assert_eq!(
        author.release_bound_book_slot(&claim, &token).await,
        Err(AuthorError::Conflict)
    );
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 0);

    sqlx::query("UPDATE chapters SET admin_hold=FALSE WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    author
        .replace_book_delayed_schedule(owner, book, true, 1, &["10:00".to_owned()])
        .await
        .unwrap();
    due(&pool, book).await;
    let next_token = "r".repeat(43);
    let next = author
        .claim_due_book_slot(&next_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&next, &next_token).await,
        Ok(BookSlotFrontier::Ready(chapter))
    );
    sqlx::query(
        "UPDATE book_delayed_publication_occurrences
         SET processing_lease_expires_at=clock_timestamp()-interval '1 second'
         WHERE book_id=$1 AND schedule_revision=$2",
    )
    .bind(book)
    .bind(next.schedule_revision)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        author.release_bound_book_slot(&next, &next_token).await,
        Err(AuthorError::Conflict),
        "expired worker cannot publish after losing lease"
    );
    let new_token = "s".repeat(43);
    let retry = author
        .claim_due_book_slot(&new_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.attempt_count, 2);
    assert_eq!(
        author.bind_book_slot_frontier(&retry, &new_token).await,
        Ok(BookSlotFrontier::Ready(chapter))
    );
    assert_eq!(
        author.release_bound_book_slot(&next, &next_token).await,
        Err(AuthorError::Conflict),
        "stale attempt cannot commit while new lease is held"
    );
    let released = author
        .release_bound_book_slot(&retry, &new_token)
        .await
        .unwrap();
    let (released_chapter, released_version) = match released {
        BookSlotReleaseOutcome::Released {
            chapter_id,
            version_id,
        } => (chapter_id, version_id),
        other => panic!("expected atomic B-06 release, got {other:?}"),
    };
    assert_eq!(released_chapter, chapter);
    let persisted_version: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(released_version, persisted_version);
    // It has completed and advanced, so the old bound occurrence cannot replay.
    let first_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(first_events, 1);
    author
        .replace_book_delayed_schedule(owner, book, false, 1, &times)
        .await
        .unwrap();
    pool.close().await;
}
