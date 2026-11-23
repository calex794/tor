//! D-001 FREE save versus concurrent author publication.
//! Disposable PostgreSQL 18 /tor_ci only.
use sqlx::{PgPool, Row};
use tor_reading::repositories::PgLibrary;

async fn chapter(pool: &PgPool, book_id: i64) -> (i64, i64) {
    let chapter_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'D001 pin race','public','free',0) RETURNING id",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'D001 exact V1') RETURNING id",
    )
    .bind(chapter_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter_id)
        .bind(v1)
        .execute(pool)
        .await
        .unwrap();
    (chapter_id, v1)
}

async fn pins(pool: &PgPool, reader: i64) -> Vec<(i64, i64)> {
    let rows = sqlx::query(
        "SELECT chapter_id,version_id
         FROM reader_saved_free_chapters WHERE user_id=$1 ORDER BY chapter_id",
    )
    .bind(reader)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|row| (row.get("chapter_id"), row.get("version_id")))
        .collect()
}

/// D-001: a bookmark that waits for an author's publication lock must
/// resolve its FREE pins after the writer commits, not silently capture
/// an obsolete edition from the statement's pre-wait MVCC snapshot.
/// This exercises the real PgLibrary::add_book SQL/lock path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "disposable PostgreSQL 18 tor_ci + GitHub CI only"]
async fn d001_bookmark_waiting_on_author_switch_pins_exact_committed_head() {
    use std::time::Duration;

    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG18");
    assert!(
        url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"),
        "D-001 must never write to persistent PostgreSQL"
    );
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D001 awaiting reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("d001-awaiting-reader-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','D001 waiting bookmark','public','basic') RETURNING id",
    )
    .bind(format!("ci:d001:bookmark-wait:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let (chapter_id, v1) = chapter(&pool, book).await;
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'D001 new exact V2') RETURNING id",
    )
    .bind(chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(v1, v2);

    // The author holds the same book-row UPDATE lock used by the actual
    // two-phase free S3 editor and commits the pointer under that lock.
    let mut writer = pool.begin().await.unwrap();
    let locked: i64 = sqlx::query_scalar("SELECT id FROM books WHERE id=$1 FOR UPDATE")
        .bind(book)
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    assert_eq!(locked, book);
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter_id)
        .bind(v2)
        .execute(&mut *writer)
        .await
        .unwrap();

    let library = PgLibrary::new(pool.clone());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let mut bookmark = tokio::spawn(async move {
        let _ = started_tx.send(());
        library.add_book(reader, book).await
    });
    started_rx.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut bookmark)
            .await
            .is_err(),
        "book save must wait for the author publication lock"
    );
    assert!(pins(&pool, reader).await.is_empty());

    writer.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), bookmark)
        .await
        .expect("bookmark must finish after publication")
        .expect("join bookstore bookmark task")
        .expect("book must be saved");

    assert_eq!(
        pins(&pool, reader).await,
        vec![(chapter_id, v2)],
        "after waiting for the author switch, save the newly committed exact edition"
    );
    let (logical, reader_retained, physical): (i64, i64, i64) = sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions,physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((logical, reader_retained, physical), (1, 1, 2));
    pool.close().await;
}
