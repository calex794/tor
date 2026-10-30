//! H-09/S023: real PostgreSQL all-or-nothing owner-only chapter import.
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::{
        authoring::{new_book, AuthorError},
        chapter_file_import::parse_text,
    },
};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn h_09_batch_import_is_atomic_private_and_idempotent() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "no production database");
    let pool = PgPool::connect(&url).await.expect("PG connect");
    let repo = PgAuthoring::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Importer','$argon2id$test') RETURNING id",
    )
    .bind(format!("h09-{nonce}@tor-ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Not importer','$argon2id$test') RETURNING id",
    )
    .bind(format!("h09-other-{nonce}@tor-ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book = new_book(
        &format!("tor-ci:h09:{nonce}"),
        "ru",
        "Сборник глав",
        &"b".repeat(43),
    )
    .unwrap();
    let book_id = repo.create_owned_book(actor, &book).await.unwrap();
    let chapters = parse_text(
        "chapters.txt",
        "# Глава 1: Начало\r\nПервый абзац\r\nГлава 2 — Вторая\r\nВторой абзац",
    )
    .unwrap();
    let key = "c".repeat(43);
    assert_eq!(
        repo.import_draft_chapter_batch(stranger, book_id, &key, &chapters)
            .await,
        Err(AuthorError::Missing),
    );
    assert_eq!(
        repo.import_draft_chapter_batch(actor, book_id, &key, &chapters)
            .await,
        Ok(2)
    );
    assert_eq!(
        repo.import_draft_chapter_batch(actor, book_id, &key, &chapters)
            .await,
        Ok(2),
        "same command replays without duplicating any chapter/version"
    );
    let changed = parse_text("chapters.txt", "Глава 1: Другое\nТекст").unwrap();
    assert_eq!(
        repo.import_draft_chapter_batch(actor, book_id, &key, &changed)
            .await,
        Err(AuthorError::Conflict),
        "same key but different content must not replay"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM chapters WHERE book_id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let versions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_versions v
         JOIN chapters c ON c.id=v.chapter_id WHERE c.book_id=$1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(versions, 2);
    let releases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_first_release_events e
         JOIN chapters c ON c.id=e.chapter_id WHERE c.book_id=$1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(releases, 0, "import is not a publication");
    let saved_body: String = sqlx::query_scalar(
        "SELECT v.body_markdown FROM chapters c
         JOIN chapter_publications p ON p.chapter_id=c.id
         JOIN chapter_versions v ON v.id=p.version_id
         WHERE c.book_id=$1 AND c.chapter_no=1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved_body, "Первый абзац");
    let visible: String = sqlx::query_scalar("SELECT visibility FROM books WHERE id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(visible, "draft");

    // First valid chapter of this new batch would insert; second conflicts.
    // PostgreSQL must roll back BOTH the new chapter and batch command.
    let with_conflict = parse_text(
        "more.md",
        "Глава 3: Новая\nТекст третьей\nГлава 1: Уже занята\nТекст повтора",
    )
    .unwrap();
    let bad_key = "d".repeat(43);
    assert_eq!(
        repo.import_draft_chapter_batch(actor, book_id, &bad_key, &with_conflict)
            .await,
        Err(AuthorError::Conflict),
    );
    let unchanged: i64 = sqlx::query_scalar("SELECT count(*) FROM chapters WHERE book_id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(unchanged, 2, "no half-import");
    let failed_batch: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_import_batches WHERE user_id=$1 AND command_key=$2",
    )
    .bind(actor)
    .bind(&bad_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failed_batch, 0, "the failed command never commits");

    repo.release_book(actor, book_id).await.unwrap();
    let future = parse_text("more.txt", "Глава 3: Новая\nТекст").unwrap();
    assert_eq!(
        repo.import_draft_chapter_batch(actor, book_id, &"e".repeat(43), &future)
            .await,
        Err(AuthorError::Conflict),
        "a public book must never auto-publish a file"
    );
    pool.close().await;
}
