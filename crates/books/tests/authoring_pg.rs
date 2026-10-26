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
    let late_key = "n".repeat(43);
    let late_chapter = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 2,
        expected_revision: 0,
        title: "Новая глава после публикации",
        body: "Не публиковать автоматически",
        access: "free",
        price_amount: 0,
        command_key: &late_key,
    })
    .unwrap();
    assert_eq!(
        author.create_free_chapter(user, &late_chapter).await,
        Err(AuthorError::Conflict),
        "saving a chapter after book release must not publish without confirmation"
    );
    let unexpected: Option<i64> =
        sqlx::query_scalar("SELECT id FROM chapters WHERE book_id=$1 AND chapter_no=2")
            .bind(book_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(unexpected.is_none(), "no partially created public chapter");
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

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn b_04_draft_chapter_cas_preserves_history_and_rejects_public_edits() {
    use tor_books::services::authoring::{save_chapter, SaveChapterRequest};

    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never use production");
    let pool = PgPool::connect(&url).await.expect("PG connect");
    let author = PgAuthoring::new(pool.clone());
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES ('b04-owner@tor-ci.example','B04 Owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES ('b04-other@tor-ci.example','B04 Other','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book = new_book(
        &format!("tor-ci:b04-draft:{user}"),
        "ru",
        "Черновик с версиями",
        &"q".repeat(43),
    )
    .unwrap();
    let book_id = author.create_owned_book(user, &book).await.unwrap();
    let first = save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Первая глава",
        body: "Исходный текст",
        access: "free",
        price_amount: 0,
        command_key: &"v".repeat(43),
    })
    .unwrap();
    let initial = author.create_free_chapter(user, &first).await.unwrap();
    let edit = |body: &str, title: &str, rev: i64, key: &str| {
        save_chapter(SaveChapterRequest {
            book_id: book_id as u64,
            chapter_id: Some(initial.chapter_id as u64),
            number: 1,
            expected_revision: rev,
            title,
            body,
            access: "free",
            price_amount: 0,
            command_key: key,
        })
        .unwrap()
    };
    let key_a = "w".repeat(43);
    let key_b = "x".repeat(43);
    let a = edit("Редакция А", "Первая глава", 1, &key_a);
    let stale = edit("Редакция Б", "Первая глава", 1, &key_b);
    assert_eq!(
        author.edit_unpublished_free_chapter(outsider, &a).await,
        Err(AuthorError::Missing)
    );
    let saved = author
        .edit_unpublished_free_chapter(user, &a)
        .await
        .unwrap();
    assert_eq!(saved.revision, 2);
    assert_ne!(saved.version_id, initial.version_id);
    assert_eq!(
        author
            .edit_unpublished_free_chapter(user, &a)
            .await
            .unwrap(),
        saved,
        "exact retry must return prior version"
    );
    let altered = edit("Подмена", "Первая глава", 1, &key_a);
    assert_eq!(
        author.edit_unpublished_free_chapter(user, &altered).await,
        Err(AuthorError::Conflict),
        "same key with changed text must conflict"
    );
    assert_eq!(
        author.edit_unpublished_free_chapter(user, &stale).await,
        Err(AuthorError::Conflict),
        "stale tab must never overwrite the winner"
    );
    let previous_body: String =
        sqlx::query_scalar("SELECT body_markdown FROM chapter_versions WHERE id=$1")
            .bind(initial.version_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        previous_body, "Исходный текст",
        "past versions are immutable"
    );

    let unchanged_key = "y".repeat(43);
    let unchanged = edit("Редакция А", "Первая глава", 2, &unchanged_key);
    assert_eq!(
        author
            .edit_unpublished_free_chapter(user, &unchanged)
            .await
            .unwrap(),
        saved,
        "saving identical text/title does not create a revision"
    );
    let title_key = "z".repeat(43);
    let title_edit = edit("Редакция А", "Новое название", 2, &title_key);
    let title_saved = author
        .edit_unpublished_free_chapter(user, &title_edit)
        .await
        .unwrap();
    assert_eq!(title_saved.revision, 3);
    assert_eq!(title_saved.version_id, saved.version_id);

    let revert_key = "u".repeat(43);
    let revert = edit("Исходный текст", "Новое название", 3, &revert_key);
    let reverted = author
        .edit_unpublished_free_chapter(user, &revert)
        .await
        .unwrap();
    assert_eq!(reverted.revision, 4);
    assert_eq!(
        reverted.version_id, initial.version_id,
        "returning to a historical body reuses its immutable version"
    );
    let version_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chapter_versions WHERE chapter_id=$1")
            .bind(initial.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version_count, 2);

    // Two simultaneous tabs submit different edits against the same revision.
    let first_race_key = "j".repeat(43);
    let second_race_key = "k".repeat(43);
    let race_a = edit("Параллельная А", "Новое название", 4, &first_race_key);
    let race_b = edit("Параллельная Б", "Новое название", 4, &second_race_key);
    let (result_a, result_b) = tokio::join!(
        author.edit_unpublished_free_chapter(user, &race_a),
        author.edit_unpublished_free_chapter(user, &race_b)
    );
    assert_eq!(
        usize::from(result_a.is_ok()) + usize::from(result_b.is_ok()),
        1,
        "only one contender may update the expected revision"
    );
    assert!(result_a == Err(AuthorError::Conflict) || result_b == Err(AuthorError::Conflict));
    let winner = result_a.or(result_b).unwrap();
    assert_eq!(winner.revision, 5);

    author.release_book(user, book_id).await.unwrap();
    let after_key = "l".repeat(43);
    let post_release = edit("Тихая замена", "Новое название", 5, &after_key);
    assert_eq!(
        author
            .edit_unpublished_free_chapter(user, &post_release)
            .await,
        Err(AuthorError::Conflict),
        "editing published pointer must require an explicit release workflow"
    );
    let public_version: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(initial.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(public_version, winner.version_id);
    let first_release_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(initial.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(first_release_count, 1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn b_01_author_genres_are_owner_written_and_retry_safe() {
    use tor_books::services::authoring::new_book_with_discovery;
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never mutate production");
    let pool = PgPool::connect(&url).await.unwrap();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES ('b01-metadata@tor-ci.example','Metadata Owner','$argon2id$test')
         ON CONFLICT (email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let key = "z".repeat(43);
    let identity = format!("ci:b01:metadata:{user}");
    let book = new_book_with_discovery(&identity, "ru", "Проза", &key, "Авторка", "Фэнтези, Роман")
        .unwrap();
    let author = PgAuthoring::new(pool.clone());
    let id = author.create_owned_book(user, &book).await.unwrap();
    assert_eq!(author.create_owned_book(user, &book).await, Ok(id));
    let row = sqlx::query(
        "SELECT author_name, genres, visibility FROM books WHERE id=$1 AND owner_user_id=$2",
    )
    .bind(id)
    .bind(user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("author_name"), "Авторка");
    assert_eq!(
        row.get::<Vec<String>, _>("genres"),
        vec!["фэнтези", "роман"]
    );
    assert_eq!(row.get::<String, _>("visibility"), "draft");
    let changed =
        new_book_with_discovery(&identity, "ru", "Проза", &key, "Подмена", "Фэнтези").unwrap();
    assert_eq!(
        author.create_owned_book(user, &changed).await,
        Err(AuthorError::Conflict)
    );
}
