//! Published books: one normal free Save emits one visible immutable edition
//! and first-release event. No confirmation page or second POST is needed.
use sqlx::PgPool;
use tor_social::repositories::notifications::PgNotifications;
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn public_free_save_is_atomic_owner_only_visible_and_idempotent() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG18 fixture");
    assert!(url.ends_with("/tor_ci"), "never modify production");
    let pool = PgPool::connect(&url).await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-save-owner@tor-ci.example','Free save owner','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-save-stranger@tor-ci.example','Free save stranger','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:free-immediate','ru','Free Save','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    // The subscriber followed the book before publication. The opted-out
    // subscriber gets an outbox receipt but never an in-app notification.
    let follower: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-save-g08-reader@tor-ci.example','G08 Reader','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let opted_out: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-save-g08-optout@tor-ci.example','G08 Opt-out','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let late_follower: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-save-g08-late@tor-ci.example','G08 Late','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$3),($2,$3)",
    )
    .bind(follower)
    .bind(opted_out)
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_notification_preferences(user_id,kind,enabled)
         VALUES($1,'chapter_created',false)",
    )
    .bind(opted_out)
    .execute(&pool)
    .await
    .unwrap();
    let authoring = PgAuthoring::new(pool.clone());
    let key = "f".repeat(43);
    let request = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 7,
        expected_revision: 0,
        title: "Новая глава",
        body: "Сохранение сразу публикует. 文 😀",
        access: "free",
        price_amount: 0,
        command_key: &key,
    })
    .unwrap();
    assert_eq!(
        authoring
            .create_published_free_chapter(stranger, &request)
            .await,
        Err(AuthorError::Missing),
        "non-owner cannot publish through the new Save path"
    );
    let saved = authoring
        .create_published_free_chapter(owner, &request)
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(
        authoring
            .create_published_free_chapter(owner, &request)
            .await,
        Ok(saved.clone()),
        "network retry is an exact replay and not another edition"
    );
    let published: (String, String, String, i64) = sqlx::query_as(
        "SELECT c.visibility,c.access_level,v.body_markdown,p.version_id
         FROM chapters c
         JOIN chapter_publications p ON p.chapter_id=c.id
         JOIN chapter_versions v ON v.id=p.version_id
         WHERE c.id=$1",
    )
    .bind(saved.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(published.0, "public");
    assert_eq!(published.1, "free");
    assert_eq!(published.2, "Сохранение сразу публикует. 文 😀");
    assert_eq!(published.3, saved.version_id);
    let release_fact: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_first_release_events
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(saved.chapter_id)
    .bind(saved.version_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(release_fact, 1);
    let published_outbox: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE event_type='ChapterPublished' AND aggregate_id=$1
           AND payload->>'version_id'=$2::text
           AND payload->>'first_release'='true'
           AND required_consumers @> ARRAY['chapter_release_fanout_v1']::text[]",
    )
    .bind(saved.chapter_id)
    .bind(saved.version_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        published_outbox, 1,
        "one committed first-release outbox event"
    );

    // Cross-component acceptance: a *real authoring transaction* must be
    // consumed by the already-running S058/E-08 contracts, never a fabricated
    // outbox fixture. A follow created after the release must not backfill.
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(late_follower)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let source_id: String = sqlx::query_scalar(
        "SELECT id::text FROM outbox_events
         WHERE event_type='ChapterPublished' AND aggregate_id=$1
           AND payload->>'version_id'=$2::text",
    )
    .bind(saved.chapter_id)
    .bind(saved.version_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    let notifier = PgNotifications::new(pool.clone());
    let mut source_done = false;
    for _ in 0..10 {
        notifier.fanout_chapter_releases(100).await.unwrap();
        source_done = sqlx::query_scalar(
            "SELECT delivered_at IS NOT NULL FROM outbox_events WHERE id=$1::uuid",
        )
        .bind(&source_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        if source_done {
            break;
        }
    }
    assert!(source_done, "real ChapterPublished source must be faned out");
    let child_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE event_type='reader.chapter.released' AND aggregate_id=$1",
    )
    .bind(saved.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(child_count, 2, "only follows at publication time are eligible");

    // Recreate the consumer instance to prove durable receipts/cursor survive.
    drop(notifier);
    let restarted = PgNotifications::new(pool.clone());
    let mut completed_receipts = 0;
    for _ in 0..10 {
        restarted.deliver_pending(100).await.unwrap();
        completed_receipts = sqlx::query_scalar(
            "SELECT count(*) FROM event_deliveries d
             JOIN outbox_events e ON e.id=d.event_id
             WHERE e.event_type='reader.chapter.released'
               AND e.aggregate_id=$1 AND d.consumer='user_notifications'",
        )
        .bind(saved.chapter_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        if completed_receipts == 2 {
            break;
        }
    }
    assert_eq!(completed_receipts, 2, "both child events must be acknowledged");
    restarted.fanout_chapter_releases(8).await.unwrap();
    let same_source_receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries
         WHERE event_id=$1::uuid AND consumer='chapter_release_fanout_v1'",
    )
    .bind(&source_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(same_source_receipts, 1, "root is acknowledged only once");
    let deliveries: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT u.id,count(n.id)
         FROM users u
         LEFT JOIN user_notifications n ON n.recipient_user_id=u.id
           AND n.kind='chapter_created' AND n.url=$1
         WHERE u.id=ANY($2::bigint[])
         GROUP BY u.id",
    )
    .bind(format!("/reader/{}", saved.chapter_id))
    .bind(&[follower, opted_out, late_follower][..])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(deliveries.len(), 3);
    for (reader, received) in deliveries {
        assert_eq!(received, if reader == follower { 1 } else { 0 });
    }
    restarted.deliver_pending(8).await.unwrap();
    let replay_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_notifications
         WHERE recipient_user_id=$1 AND kind='chapter_created' AND url=$2",
    )
    .bind(follower)
    .bind(format!("/reader/{}", saved.chapter_id))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(replay_count, 1, "restart/replay cannot duplicate notifications");

    let working: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_working_drafts WHERE chapter_id=$1")
            .bind(saved.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(working, 0);

    let modified = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 7,
        expected_revision: 0,
        title: "Взлом повтора",
        body: "Новый текст под старым ключом",
        access: "free",
        price_amount: 0,
        command_key: &key,
    })
    .unwrap();
    assert_eq!(
        authoring
            .create_published_free_chapter(owner, &modified)
            .await,
        Err(AuthorError::Conflict),
        "same key cannot replace an immutable edition"
    );
    let duplicate_key = "g".repeat(43);
    let duplicate_number = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 7,
        expected_revision: 0,
        title: "Другая глава №7",
        body: "Не должно появиться второй главы с тем же номером",
        access: "free",
        price_amount: 0,
        command_key: &duplicate_key,
    })
    .unwrap();
    assert_eq!(
        authoring
            .create_published_free_chapter(owner, &duplicate_number)
            .await,
        Err(AuthorError::Conflict)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM chapters WHERE book_id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn draft_book_is_still_private_and_public_save_fails_closed() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG18 fixture");
    assert!(url.ends_with("/tor_ci"), "never modify production");
    let pool = PgPool::connect(&url).await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('free-save-draft@tor-ci.example','Draft book owner','$argon2id$test')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:free-immediate-draft','ru','Draft stays private','draft',$1)
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let command = "d".repeat(43);
    let input = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Скрытая глава",
        body: "Ещё не выпущенная книга",
        access: "free",
        price_amount: 0,
        command_key: &command,
    })
    .unwrap();
    let author = PgAuthoring::new(pool.clone());
    assert_eq!(
        author.create_published_free_chapter(owner, &input).await,
        Err(AuthorError::Conflict),
        "Save must not publish from an unpublished book"
    );
    let hidden = author.create_free_chapter(owner, &input).await.unwrap();
    let visibility: String = sqlx::query_scalar("SELECT visibility FROM books WHERE id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(visibility, "draft");
    let version: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(hidden.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version, hidden.version_id);
    pool.close().await;
}
