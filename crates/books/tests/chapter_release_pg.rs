//! B-06: a public book may have private free chapters until explicit release.
use sqlx::PgPool;
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, save_chapter, AuthorError, SaveChapterRequest},
};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn private_chapter_is_never_public_until_exact_version_release() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never run against production");
    let pool = PgPool::connect(&url).await.expect("PostgreSQL");
    let author = PgAuthoring::new(pool.clone());
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('b06-owner@tor-ci.example','B06 Owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('b06-outsider@tor-ci.example','B06 Outsider','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book = new_book(
        &format!("tor-ci:b06:{owner}"),
        "ru",
        "Книга уже опубликована",
        &"a".repeat(43),
    )
    .unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    let first = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Первая глава",
        body: "Открытая глава",
        access: "free",
        price_amount: 0,
        command_key: &"b".repeat(43),
    })
    .unwrap();
    author.create_free_chapter(owner, &first).await.unwrap();
    author.release_book(owner, book_id).await.unwrap();

    let saved_input = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 2,
        expected_revision: 0,
        title: "Вторая глава",
        body: "СКРЫТО: первый черновик",
        access: "free",
        price_amount: 0,
        command_key: &"c".repeat(43),
    })
    .unwrap();
    assert_eq!(
        author.create_pending_free_chapter(outsider, &saved_input).await,
        Err(AuthorError::Missing)
    );
    let saved = author
        .create_pending_free_chapter(owner, &saved_input)
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(
        author.create_pending_free_chapter(owner, &saved_input).await,
        Ok(saved.clone()),
        "a replay cannot create a second chapter"
    );
    let tampered = save_chapter(SaveChapterRequest {
        body: "Подмена команды",
        ..SaveChapterRequest {
            book_id: book_id as u64,
            chapter_id: None,
            number: 2,
            expected_revision: 0,
            title: "Вторая глава",
            body: "СКРЫТО: первый черновик",
            access: "free",
            price_amount: 0,
            command_key: &"c".repeat(43),
        }
    })
    .unwrap();
    assert_eq!(
        author.create_pending_free_chapter(owner, &tampered).await,
        Err(AuthorError::Conflict)
    );
    let publication: Option<i64> = sqlx::query_scalar(
        "SELECT version_id FROM chapter_publications WHERE chapter_id=$1",
    )
    .bind(saved.chapter_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(publication.is_none(), "draft never has a reader pointer");
    let accessible: Option<i64> = sqlx::query_scalar(
        "SELECT c.id FROM chapters c JOIN books b ON b.id=c.book_id
         JOIN chapter_publications p ON p.chapter_id=c.id
         WHERE c.id=$1 AND b.visibility='public' AND c.visibility='public'",
    )
    .bind(saved.chapter_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(accessible.is_none(), "not readable even if parent is public");
    assert!(
        author
            .pending_free_chapter(outsider, book_id, saved.chapter_id)
            .await
            .unwrap()
            .is_none(),
        "outsider cannot inspect unpublished text"
    );
    let pending = author
        .pending_free_chapter(owner, book_id, saved.chapter_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.version_id, saved.version_id);
    assert_eq!(pending.revision, 1);

    let edited_input = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: Some(saved.chapter_id as u64),
        number: 2,
        expected_revision: 1,
        title: "Вторая глава — исправлена",
        body: "СКРЫТО: проверенный текст",
        access: "free",
        price_amount: 0,
        command_key: &"d".repeat(43),
    })
    .unwrap();
    let updated = author
        .edit_unpublished_free_chapter(owner, &edited_input)
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_ne!(updated.version_id, saved.version_id);
    assert_eq!(
        author
            .release_pending_free_chapter(owner, book_id, saved.chapter_id, 1, saved.version_id)
            .await,
        Err(AuthorError::Conflict),
        "an old confirmation cannot publish stale text"
    );
    assert_eq!(
        author
            .release_pending_free_chapter(outsider, book_id, saved.chapter_id, 2, updated.version_id)
            .await,
        Err(AuthorError::Missing)
    );

    // The book row lock serializes both explicit release submissions.
    let (a, b) = tokio::join!(
        author.release_pending_free_chapter(owner, book_id, saved.chapter_id, 2, updated.version_id),
        author.release_pending_free_chapter(owner, book_id, saved.chapter_id, 2, updated.version_id)
    );
    assert_eq!(a, Ok(()));
    assert_eq!(b, Ok(()));
    let visible: Option<String> = sqlx::query_scalar(
        "SELECT v.body_markdown FROM chapters c JOIN books b ON b.id=c.book_id
         JOIN chapter_publications p ON p.chapter_id=c.id
         JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
         WHERE c.id=$1 AND b.visibility='public' AND c.visibility='public'
           AND c.access_level='free' AND NOT b.admin_hold AND NOT c.admin_hold",
    )
    .bind(saved.chapter_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(visible.as_deref(), Some("СКРЫТО: проверенный текст"));
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chapter_first_release_events WHERE chapter_id=$1",
    )
    .bind(saved.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event_count, 1, "only one durable first-release fact");
    let draft_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chapter_working_drafts WHERE chapter_id=$1",
    )
    .bind(saved.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(draft_count, 0, "working pointer is removed on publication");
    assert!(
        author
            .pending_free_chapter(owner, book_id, saved.chapter_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        author.edit_unpublished_free_chapter(owner, &edited_input).await,
        Err(AuthorError::Conflict),
        "published version cannot be changed through the draft editor"
    );
    let original_body: String =
        sqlx::query_scalar("SELECT body_markdown FROM chapter_versions WHERE id=$1")
            .bind(saved.version_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(original_body, "СКРЫТО: первый черновик");
    pool.close().await;
}
