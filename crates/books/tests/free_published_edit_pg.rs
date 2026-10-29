//! BKR-96: normal Save instantly activates a new immutable FREE chapter
//! version in a published book. Prove owner ACL, CAS, replay and two-tab race
//! against real disposable PostgreSQL 18, not a fake transaction.
use sqlx::PgPool;
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest, SavedChapter},
};

fn update(
    book: i64,
    chapter: i64,
    expected_revision: i64,
    title: &str,
    body: &str,
    key: &str,
) -> tor_books::services::authoring::SaveChapter {
    save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: Some(chapter as u64),
        number: 7,
        expected_revision,
        title,
        body,
        access: "free",
        price_amount: 0,
        command_key: key,
    })
    .unwrap()
}

async fn fixture(pool: &PgPool, label: &str) -> (PgAuthoring, i64, i64, i64, SavedChapter) {
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Published editor owner','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("bkr96-{label}-owner@tor-ci.example"))
    .fetch_one(pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Another author','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("bkr96-{label}-stranger@tor-ci.example"))
    .fetch_one(pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Published free chapter','public',$2) RETURNING id",
    )
    .bind(format!("ci:bkr96:{label}"))
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap();
    let author = PgAuthoring::new(pool.clone());
    let original_key = "a".repeat(43);
    let initial = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 7,
        expected_revision: 0,
        title: "Исходная глава",
        body: "Оригинальный текст V1 😀",
        access: "free",
        price_amount: 0,
        command_key: &original_key,
    })
    .unwrap();
    let original = author
        .create_published_free_chapter(owner, &initial)
        .await
        .unwrap();
    (author, owner, stranger, book, original)
}

#[tokio::test]
#[ignore = "only disposable PostgreSQL 18 TEST_DATABASE_URL ending /tor_ci"]
async fn bkr96_published_free_edit_is_visible_immediately_and_old_version_is_immutable() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated CI PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never write production");
    let pool = PgPool::connect(&url).await.unwrap();
    let (author, owner, stranger, book, initial) = fixture(&pool, "success").await;
    assert_eq!(
        author
            .owned_published_free_chapter(stranger, book, initial.chapter_id)
            .await
            .unwrap()
            .is_some(),
        false
    );
    let owned = author
        .owned_published_free_chapter(owner, book, initial.chapter_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owned.revision, 1);
    assert_eq!(owned.body, "Оригинальный текст V1 😀");
    let key = "b".repeat(43);
    let saved_input = update(
        book,
        initial.chapter_id,
        1,
        "Исправленная глава",
        "Новый текст V2\n文 😀",
        &key,
    );
    assert_eq!(
        author
            .edit_published_free_chapter(stranger, &saved_input)
            .await,
        Err(AuthorError::Missing),
        "non-owner cannot edit public content"
    );
    let saved = author
        .edit_published_free_chapter(owner, &saved_input)
        .await
        .unwrap();
    assert_eq!(saved.revision, 2);
    assert_ne!(saved.version_id, initial.version_id);
    let current: (i64, String, String, i64) = sqlx::query_as(
        "SELECT p.version_id,c.title,v.body_markdown,c.edit_revision
         FROM chapters c
         JOIN chapter_publications p ON p.chapter_id=c.id
         JOIN chapter_versions v ON v.id=p.version_id
         WHERE c.id=$1",
    )
    .bind(initial.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        current,
        (
            saved.version_id,
            "Исправленная глава".to_owned(),
            "Новый текст V2\n文 😀".to_owned(),
            2
        )
    );
    let old_body: String =
        sqlx::query_scalar("SELECT body_markdown FROM chapter_versions WHERE id=$1")
            .bind(initial.version_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old_body, "Оригинальный текст V1 😀");
    let first: (i64, i64) = sqlx::query_as(
        "SELECT chapter_id,version_id FROM chapter_first_release_events WHERE chapter_id=$1",
    )
    .bind(initial.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first, (initial.chapter_id, initial.version_id));
    let unchanged_working: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_working_drafts WHERE chapter_id=$1")
            .bind(initial.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unchanged_working, 0);

    // Lost HTTP response: same command/key gives exact original result.
    assert_eq!(
        author
            .edit_published_free_chapter(owner, &saved_input)
            .await,
        Ok(saved.clone()),
    );
    let altered_payload = update(
        book,
        initial.chapter_id,
        1,
        "Фальшивая глава",
        "Украденная команда",
        &key,
    );
    assert_eq!(
        author
            .edit_published_free_chapter(owner, &altered_payload)
            .await,
        Err(AuthorError::Conflict)
    );
    let stale = update(
        book,
        initial.chapter_id,
        1,
        "Вкладка 2",
        "Старые данные",
        &"c".repeat(43),
    );
    assert_eq!(
        author.edit_published_free_chapter(owner, &stale).await,
        Err(AuthorError::Conflict)
    );
    let current_after_conflict: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(initial.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current_after_conflict, saved.version_id);

    // Same published content with a new command is a no-op, not another
    // immutable version or reader change.
    let noop = update(
        book,
        initial.chapter_id,
        2,
        "Исправленная глава",
        "Новый текст V2\n文 😀",
        &"d".repeat(43),
    );
    assert_eq!(
        author.edit_published_free_chapter(owner, &noop).await,
        Ok(saved.clone())
    );
    let version_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_versions WHERE chapter_id=$1")
            .bind(initial.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version_count, 2);
    pool.close().await;
}

#[tokio::test]
#[ignore = "only disposable PostgreSQL 18 TEST_DATABASE_URL ending /tor_ci"]
async fn bkr96_two_edit_tabs_conflict_and_reverted_text_reuses_old_version() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated CI PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never write production");
    let pool = PgPool::connect(&url).await.unwrap();
    let (author, owner, _stranger, book, initial) = fixture(&pool, "race").await;
    let first = update(
        book,
        initial.chapter_id,
        1,
        "Первая вкладка",
        "Текст А",
        &"e".repeat(43),
    );
    let second = update(
        book,
        initial.chapter_id,
        1,
        "Вторая вкладка",
        "Текст Б",
        &"f".repeat(43),
    );
    let (left, right) = tokio::join!(
        author.edit_published_free_chapter(owner, &first),
        author.edit_published_free_chapter(owner, &second)
    );
    assert!(
        (left.is_ok() && right == Err(AuthorError::Conflict))
            || (right.is_ok() && left == Err(AuthorError::Conflict)),
        "one live revision wins, stale concurrent writer gets 409"
    );
    let winner = left.or(right).unwrap();
    assert_eq!(winner.revision, 2);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_versions WHERE chapter_id=$1")
            .bind(initial.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2, "losing transaction leaves no extra version");
    let revert = update(
        book,
        initial.chapter_id,
        2,
        "Вернули первую редакцию",
        "Оригинальный текст V1 😀",
        &"g".repeat(43),
    );
    let restored = author
        .edit_published_free_chapter(owner, &revert)
        .await
        .unwrap();
    assert_eq!(restored.version_id, initial.version_id);
    assert_eq!(restored.revision, 3);
    let after_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_versions WHERE chapter_id=$1")
            .bind(initial.chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after_count, 2, "revert reuses historical immutable body");
    let first_release: i64 = sqlx::query_scalar(
        "SELECT version_id FROM chapter_first_release_events WHERE chapter_id=$1",
    )
    .bind(initial.chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first_release, initial.version_id);
    pool.close().await;
}

#[tokio::test]
#[ignore = "only disposable PostgreSQL 18 TEST_DATABASE_URL ending /tor_ci"]
async fn bkr96_legacy_published_free_revision_zero_is_editable_without_second_release() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated CI PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never write production");
    let pool = PgPool::connect(&url).await.unwrap();
    let (author, owner, _stranger, book, initial) = fixture(&pool, "legacy").await;
    // Imported free versions can have edit_revision=0 before author editing.
    sqlx::query("UPDATE chapters SET edit_revision=0 WHERE id=$1")
        .bind(initial.chapter_id)
        .execute(&pool)
        .await
        .unwrap();
    let old = author
        .owned_published_free_chapter(owner, book, initial.chapter_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.revision, 0);
    let payload = update(
        book,
        initial.chapter_id,
        0,
        "Первая правка после переноса",
        "Старая редакция сохранена",
        &"h".repeat(43),
    );
    let changed = author
        .edit_published_free_chapter(owner, &payload)
        .await
        .unwrap();
    assert_eq!(changed.revision, 1);
    assert_ne!(changed.version_id, initial.version_id);
    let original: String =
        sqlx::query_scalar("SELECT body_markdown FROM chapter_versions WHERE id=$1")
            .bind(initial.version_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(original, "Оригинальный текст V1 😀");
    pool.close().await;
}
