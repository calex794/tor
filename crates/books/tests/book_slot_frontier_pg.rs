//! S058: first invisible chapter wins; terminal blocked/empty are durable.
//! Only disposable /tor_ci PostgreSQL may be modified.
use sqlx::{Connection, PgPool};
use tor_books::{
    repositories::postgres::{book_delayed_schedule::BookSlotFrontier, PgAuthoring},
    services::authoring::{new_book, save_chapter, AuthorError, SaveChapterRequest},
};

// Browser commands are scoped by (user, key), not by book. Repeated
// disposable fixtures for the same owner therefore need distinct keys.
fn fixture_key(book: i64, chapter_no: i32) -> String {
    format!("{:043x}", ((book as u128) << 32) | (chapter_no as u128))
}

fn chapter<'a>(
    book: i64,
    number: i32,
    title: &'a str,
    key: &'a str,
) -> tor_books::services::authoring::SaveChapter {
    save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number,
        expected_revision: 0,
        title,
        body: "Версия главы для проверки фронтира",
        access: "free",
        price_amount: 0,
        command_key: key,
    })
    .unwrap()
}

async fn owner_and_book(pool: &PgPool, suffix: &str) -> (PgAuthoring, i64, i64) {
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Frontier owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .bind(format!("s058-frontier-{suffix}@tor-ci.example"))
    .fetch_one(pool)
    .await
    .unwrap();
    let author = PgAuthoring::new(pool.clone());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let book = new_book(
        &format!("tor-ci:s058-frontier:{suffix}:{owner}:{nonce}"),
        "ru",
        &format!("Проверка очереди {suffix} {nonce}"),
        &format!("{nonce:043x}"),
    )
    .unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    let first = chapter(book_id, 1, "Опубликованная глава", &fixture_key(book_id, 1));
    author.create_free_chapter(owner, &first).await.unwrap();
    author.release_book(owner, book_id).await.unwrap();
    (author, owner, book_id)
}

async fn force_due(pool: &PgPool, book: i64) {
    sqlx::query(
        "UPDATE book_delayed_publication_settings
         SET next_slot_at=clock_timestamp()-interval '1 second'
         WHERE book_id=$1",
    )
    .bind(book)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_blocked_frontier_never_skips_and_ready_binding_is_fenced() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never run on production");
    // A database-scoped fixture lock coordinates separate Cargo test binaries.
    // Dropping this connection releases the lock, including on panic.
    let mut slot_fixture_lock = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(580058)")
        .execute(&mut slot_fixture_lock)
        .await
        .unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let (author, owner, book) = owner_and_book(&pool, "blocked").await;
    let two = author
        .create_pending_free_chapter(owner, &chapter(book, 2, "Вторая", &fixture_key(book, 2)))
        .await
        .unwrap();
    let three = author
        .create_pending_free_chapter(owner, &chapter(book, 3, "Третья", &fixture_key(book, 3)))
        .await
        .unwrap();
    let times = ["09:00".to_owned(), "18:00".to_owned()];
    author
        .replace_book_delayed_schedule(owner, book, true, 2, &times)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET admin_hold=TRUE WHERE id=$1")
        .bind(two.chapter_id)
        .execute(&pool)
        .await
        .unwrap();
    force_due(&pool, book).await;
    let token = "q".repeat(43);
    let first = author.claim_due_book_slot(&token).await.unwrap().unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&first, &token).await,
        Ok(BookSlotFrontier::Blocked {
            chapter_id: Some(two.chapter_id),
            reason: "chapter_admin_hold",
        })
    );
    let recorded: (String, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT status,chapter_id,last_error_code
         FROM book_delayed_publication_occurrences WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recorded.0, "blocked");
    assert_eq!(recorded.1, Some(two.chapter_id));
    assert_eq!(recorded.2.as_deref(), Some("chapter_admin_hold"));
    assert!(author
        .claim_due_book_slot(&"r".repeat(43))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        author.bind_book_slot_frontier(&first, &token).await,
        Err(AuthorError::Conflict)
    );
    assert!(author
        .scheduled_free_chapter(owner, book, three.chapter_id)
        .await
        .unwrap()
        .is_none());

    // A repaired frontier can be chosen on the NEW schedule revision only.
    sqlx::query("UPDATE chapters SET admin_hold=FALSE WHERE id=$1")
        .bind(two.chapter_id)
        .execute(&pool)
        .await
        .unwrap();
    let revised = author
        .replace_book_delayed_schedule(
            owner,
            book,
            true,
            2,
            &["10:00".to_owned(), "18:00".to_owned()],
        )
        .await
        .unwrap();
    assert_eq!(revised.schedule_revision, 2);
    force_due(&pool, book).await;
    let next_token = "s".repeat(43);
    let next = author
        .claim_due_book_slot(&next_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.schedule_revision, 2);
    assert_eq!(
        author.bind_book_slot_frontier(&next, &next_token).await,
        Ok(BookSlotFrontier::Ready(two.chapter_id))
    );
    assert_eq!(
        author.bind_book_slot_frontier(&next, &next_token).await,
        Ok(BookSlotFrontier::Ready(two.chapter_id))
    );
    assert_eq!(
        author.bind_book_slot_frontier(&next, &"t".repeat(43)).await,
        Err(AuthorError::Conflict)
    );
    let bound: Option<i64> = sqlx::query_scalar(
        "SELECT chapter_id FROM book_delayed_publication_occurrences
         WHERE book_id=$1 AND schedule_revision=2",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(bound, Some(two.chapter_id));
    let publications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_publications
         WHERE chapter_id IN ($1,$2)",
    )
    .bind(two.chapter_id)
    .bind(three.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(publications, 0, "binding cannot disclose draft chapters");
    let facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_first_release_events
         WHERE chapter_id IN ($1,$2)",
    )
    .bind(two.chapter_id)
    .bind(three.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(facts, 0);
    author
        .replace_book_delayed_schedule(owner, book, false, 2, &times)
        .await
        .unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&next, &next_token).await,
        Err(AuthorError::Conflict),
        "a disabled schedule invalidates the old worker"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_empty_slot_advances_duplicate_time_and_paid_frontier_blocks() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never run on production");
    // A database-scoped fixture lock coordinates separate Cargo test binaries.
    // Dropping this connection releases the lock, including on panic.
    let mut slot_fixture_lock = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(580058)")
        .execute(&mut slot_fixture_lock)
        .await
        .unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let (author, owner, book) = owner_and_book(&pool, "empty").await;
    author
        .replace_book_delayed_schedule(
            owner,
            book,
            true,
            1,
            &["09:00".to_owned(), "09:00".to_owned()],
        )
        .await
        .unwrap();
    force_due(&pool, book).await;
    let token = "v".repeat(43);
    let claim = author.claim_due_book_slot(&token).await.unwrap().unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&claim, &token).await,
        Ok(BookSlotFrontier::Empty)
    );
    let cursor: i32 = sqlx::query_scalar(
        "SELECT next_slot_index FROM book_delayed_publication_settings WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cursor, 1, "duplicate wall-clock times are distinct slots");
    let terminal: String = sqlx::query_scalar(
        "SELECT status FROM book_delayed_publication_occurrences WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(terminal, "empty");
    // The next duplicate-time slot may already be due; prevent a second
    // fixture from taking this book's slot through the global worker claim.
    author
        .replace_book_delayed_schedule(
            owner,
            book,
            false,
            1,
            &["09:00".to_owned(), "09:00".to_owned()],
        )
        .await
        .unwrap();

    let (author, owner, paid_book) = owner_and_book(&pool, "paid").await;
    let paid: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,
                              price_amount,edit_revision)
         VALUES($1,2,'Платная скрытая глава','draft','paid',175,1)
         RETURNING id",
    )
    .bind(paid_book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let later = author
        .create_pending_free_chapter(
            owner,
            &chapter(
                paid_book,
                3,
                "Бесплатная после платной",
                &fixture_key(paid_book, 3),
            ),
        )
        .await
        .unwrap();
    author
        .replace_book_delayed_schedule(owner, paid_book, true, 1, &["09:00".to_owned()])
        .await
        .unwrap();
    force_due(&pool, paid_book).await;
    let token = "x".repeat(43);
    let claim = author.claim_due_book_slot(&token).await.unwrap().unwrap();
    assert_eq!(
        author.bind_book_slot_frontier(&claim, &token).await,
        Ok(BookSlotFrontier::Blocked {
            chapter_id: Some(paid),
            reason: "paid_version_not_prepared",
        })
    );
    let later_publication: Option<i64> =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(later.chapter_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(later_publication, None);
    pool.close().await;
}
