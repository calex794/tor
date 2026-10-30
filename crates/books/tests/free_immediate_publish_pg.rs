//! Published books: one normal free Save emits one visible immutable edition
//! and first-release event. No confirmation page or second POST is needed.
use sqlx::PgPool;
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
    let outbox: (i64, bool) = sqlx::query_as(
        "SELECT COUNT(*)::bigint,
                BOOL_AND((payload->>'first_release')::boolean)
         FROM outbox_events
         WHERE event_type='ChapterPublished' AND aggregate_id=$1",
    )
    .bind(saved.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(outbox, (1, true), "exact command replay emits one event");
    let envelope: (String, i64, i64, Vec<String>, bool) = sqlx::query_as(
        "SELECT dedupe_key,(payload->>'book_id')::bigint,
                (payload->>'version_id')::bigint,
                required_consumers,payload ? 'published_at'
         FROM outbox_events
         WHERE event_type='ChapterPublished' AND aggregate_id=$1",
    )
    .bind(saved.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        envelope.0,
        format!(
            "chapter-published:{}:{}",
            saved.chapter_id, saved.version_id
        )
    );
    assert_eq!((envelope.1, envelope.2), (book, saved.version_id));
    assert_eq!(envelope.3, vec!["search-index-v1", "subscription-inbox-v1"]);
    assert!(envelope.4, "published_at is part of the version envelope");
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
