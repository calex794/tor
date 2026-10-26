//! B-01/B-02: a real owner creates one draft, protected against retries.
use sqlx::{PgPool, Row};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, AuthorError},
};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn b_01_owner_draft_is_private_and_idempotent() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never run on production");
    let pool = PgPool::connect(&url).await.expect("PG connect");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized, display_name, password_hash)
         VALUES('b01_owner@tor-ci.example','B01 Owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("fixture");
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized, display_name, password_hash)
         VALUES('b01_other@tor-ci.example','B01 Other','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("fixture");
    let author = PgAuthoring::new(pool.clone());
    let key = "b".repeat(43);
    let identity = format!("tor-ci:b01:{user}");
    let draft = new_book(&identity, "ru", "Первый черновик", &key).unwrap();
    let book_id = author.create_owned_book(user, &draft).await.unwrap();
    assert_eq!(
        author.create_owned_book(user, &draft).await.unwrap(),
        book_id
    );
    assert_eq!(
        author
            .owned_book(user, book_id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "Первый черновик"
    );
    assert!(author.owned_book(other, book_id).await.unwrap().is_none());
    let visible: Option<i64> =
        sqlx::query_scalar("SELECT id FROM books WHERE id=$1 AND visibility='public'")
            .bind(book_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(visible.is_none(), "new books must stay drafts");
    let changed = new_book(&identity, "ru", "Подменённый заголовок", &key).unwrap();
    assert_eq!(
        author.create_owned_book(user, &changed).await,
        Err(AuthorError::Conflict)
    );
    let distinct_key = "c".repeat(43);
    let duplicate = new_book(&identity, "ru", "Первый черновик", &distinct_key).unwrap();
    assert_eq!(
        author.create_owned_book(user, &duplicate).await,
        Err(AuthorError::Conflict),
        "canonical identity must be unique"
    );
    let row = sqlx::query("SELECT owner_user_id,visibility FROM books WHERE id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("owner_user_id"), user);
    assert_eq!(row.get::<String, _>("visibility"), "draft");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn b_03_04_free_chapter_then_deliberate_book_release() {
    use tor_books::services::authoring::{save_chapter, SaveChapterRequest};

    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never run on production");
    let pool = PgPool::connect(&url).await.expect("PG connect");
    let author = PgAuthoring::new(pool.clone());
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES ('b03-owner@tor-ci.example','Chapter Owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("owner fixture");
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES ('b03-outsider@tor-ci.example','Outsider','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("outsider fixture");
    let book_key = "k".repeat(43);
    let chapter_key = "m".repeat(43);
    let book = new_book(
        &format!("tor-ci:b03:{user}"),
        "ru",
        "Реальная книга",
        &book_key,
    )
    .unwrap();
    let book_id = author.create_owned_book(user, &book).await.unwrap();
    assert_eq!(
        author.release_book(user, book_id).await,
        Err(AuthorError::Conflict),
        "an empty book cannot be published"
    );
    let first = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Первая глава",
        body: "Реальный Markdown-текст",
        access: "free",
        price_amount: 0,
        command_key: &chapter_key,
    })
    .unwrap();
    assert_eq!(
        author.create_free_chapter(outsider, &first).await,
        Err(AuthorError::Missing),
        "another account cannot add a chapter"
    );
    let saved = author.create_free_chapter(user, &first).await.unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(
        author.create_free_chapter(user, &first).await.unwrap(),
        saved,
        "exact replay must return the existing chapter/version"
    );
    let altered = save_chapter(SaveChapterRequest {
        body: "Изменённый текст",
        ..SaveChapterRequest {
            book_id: book_id as u64,
            chapter_id: None,
            number: 1,
            expected_revision: 0,
            title: "Первая глава",
            body: "Реальный Markdown-текст",
            access: "free",
            price_amount: 0,
            command_key: &chapter_key,
        }
    })
    .unwrap();
    assert_eq!(
        author.create_free_chapter(user, &altered).await,
        Err(AuthorError::Conflict),
        "same command key with different body cannot silently overwrite"
    );
    let version_body: String =
        sqlx::query_scalar("SELECT body_markdown FROM chapter_versions WHERE id=$1")
            .bind(saved.version_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version_body, "Реальный Markdown-текст");
    let book_status: String = sqlx::query_scalar("SELECT visibility FROM books WHERE id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_status, "draft", "saving a chapter is not book release");
    let released_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(saved.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        released_events, 0,
        "hidden book has no public release event"
    );
    assert_eq!(
        author.release_book(outsider, book_id).await,
        Err(AuthorError::Missing),
        "only the owner can release"
    );
    author.release_book(user, book_id).await.unwrap();
    author.release_book(user, book_id).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(saved.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "one first release event, even on repeat");
    let public_title: Option<String> = sqlx::query_scalar(
        "SELECT b.title FROM books b
         JOIN chapters c ON c.book_id=b.id
         JOIN chapter_publications p ON p.chapter_id=c.id
         JOIN chapter_versions v ON v.id=p.version_id
         WHERE b.id=$1 AND b.visibility='public' AND NOT b.admin_hold
           AND c.visibility='public' AND c.access_level='free'
           AND v.body_markdown='Реальный Markdown-текст'",
    )
    .bind(book_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(public_title.as_deref(), Some("Реальная книга"));
    pool.close().await;
}
