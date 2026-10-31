//! S023 B finalizer: lease fencing and cancellation share the chapter transaction.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::{
        authoring::{new_book, AuthorError},
        chapter_file_import::{fingerprint, parse_text},
    },
};

async fn insert_running_job(
    pool: &PgPool,
    actor: i64,
    book: i64,
    command_key: &str,
    filename: &str,
    text: &str,
    approved: &[u8; 32],
) -> i64 {
    let source_hash: [u8; 32] = Sha256::digest(text.as_bytes()).into();
    let source_key = format!("staging/chapters/{actor}/{book}/{command_key}");
    sqlx::query_scalar(
        "INSERT INTO chapter_import_jobs
         (book_id,actor_user_id,command_key,source_key,source_filename,
          source_sha256,source_bytes,approved_fingerprint,expected_chapter_count,
          status,lease_epoch,locked_until)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,1,'running',1,now()+INTERVAL '120 seconds')
         RETURNING id",
    )
    .bind(book)
    .bind(actor)
    .bind(command_key)
    .bind(source_key)
    .bind(filename)
    .bind(source_hash.as_slice())
    .bind(text.len() as i64)
    .bind(approved.as_slice())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn s023_b_finalizer_fences_lease_and_cancel_atomically() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "no production database");
    let pool = PgPool::connect(&url).await.unwrap();
    let repo = PgAuthoring::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Importer','$argon2id$test') RETURNING id",
    )
    .bind(format!("s023-b-finalize-{nonce}@tor-ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book = new_book(
        &format!("tor-ci:s023-b-finalize:{nonce}"),
        "ru",
        "Atomic import",
        &"b".repeat(43),
    )
    .unwrap();
    let book_id = repo.create_owned_book(actor, &book).await.unwrap();

    let first_text = "Глава 1: Первая\nТекст первой";
    let first_chapters = parse_text("first.txt", first_text).unwrap();
    let first_fingerprint = fingerprint(book_id, &first_chapters);
    let first_key = "c".repeat(43);
    let first_job = insert_running_job(
        &pool,
        actor,
        book_id,
        &first_key,
        "first.txt",
        first_text,
        &first_fingerprint,
    )
    .await;
    assert_eq!(
        repo.complete_chapter_import_job(first_job, 1, &first_chapters)
            .await,
        Ok(1)
    );
    assert_eq!(
        repo.import_draft_chapter_batch(actor, book_id, &first_key, &first_chapters)
            .await,
        Ok(1),
        "replaying the batch command cannot create a duplicate chapter"
    );

    // A replacement worker owns epoch 2. The old epoch must not write anything.
    let second_text = "Глава 2: Вторая\nТекст второй";
    let second_chapters = parse_text("second.txt", second_text).unwrap();
    let second_fingerprint = fingerprint(book_id, &second_chapters);
    let second_key = "d".repeat(43);
    let second_job = insert_running_job(
        &pool,
        actor,
        book_id,
        &second_key,
        "second.txt",
        second_text,
        &second_fingerprint,
    )
    .await;
    sqlx::query("UPDATE chapter_import_jobs SET lease_epoch=2 WHERE id=$1")
        .bind(second_job)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.complete_chapter_import_job(second_job, 1, &second_chapters)
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        repo.complete_chapter_import_job(second_job, 2, &second_chapters)
            .await,
        Ok(1)
    );

    // Cancellation and finalization race on the same job row. Exactly one wins.
    let third_text = "Глава 3: Третья\nТекст третьей";
    let third_chapters = parse_text("third.txt", third_text).unwrap();
    let third_fingerprint = fingerprint(book_id, &third_chapters);
    let third_key = "e".repeat(43);
    let third_job = insert_running_job(
        &pool,
        actor,
        book_id,
        &third_key,
        "third.txt",
        third_text,
        &third_fingerprint,
    )
    .await;
    let finalize = repo.complete_chapter_import_job(third_job, 1, &third_chapters);
    let cancel = sqlx::query(
        "UPDATE chapter_import_jobs SET status='cancelled',locked_until=NULL,
             lease_epoch=lease_epoch+1,finished_at=now(),updated_at=now()
         WHERE id=$1 AND status IN ('queued','running') RETURNING id",
    )
    .bind(third_job)
    .fetch_optional(&pool);
    let (completed, cancelled) = tokio::join!(finalize, cancel);
    let cancelled = cancelled.unwrap().is_some();
    let status: String =
        sqlx::query_scalar("SELECT status FROM chapter_import_jobs WHERE id=$1")
            .bind(third_job)
            .fetch_one(&pool)
            .await
            .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM chapters WHERE book_id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    match (completed, cancelled, status.as_str(), count) {
        (Ok(1), false, "succeeded", 3) => {}
        (Err(AuthorError::Conflict), true, "cancelled", 2) => {}
        other => panic!("cancel/finalize was not atomic: {other:?}"),
    }
    pool.close().await;
}
