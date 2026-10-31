//! S023: disposable PostgreSQL 18 regression for durable queue claims and atomic finalization.
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn s023_claim_fencing_cancel_and_private_status() {
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
    .bind(format!("s023-job-{nonce}@tor-ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Stranger','$argon2id$test') RETURNING id",
    )
    .bind(format!("s023-stranger-{nonce}@tor-ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book = new_book(
        &format!("tor-ci:s023-jobs:{nonce}"),
        "ru",
        "Очередь",
        &"b".repeat(43),
    )
    .unwrap();
    let book_id = repo.create_owned_book(actor, &book).await.unwrap();

    // Cancellation wins before finalization: the worker's old epoch is fenced.
    let cancelled_text = "Глава 1: Отменённая\nТекст";
    let cancelled_chapters = parse_text("cancelled.txt", cancelled_text).unwrap();
    let cancelled_hash: [u8; 32] = Sha256::digest(cancelled_text.as_bytes()).into();
    let cancelled_fingerprint = fingerprint(book_id, &cancelled_chapters);
    let cancelled_key = "c".repeat(43);
    let cancelled_job = repo
        .enqueue_verified_chapter_import(
            actor,
            book_id,
            &cancelled_key,
            &format!("staging/chapters/{actor}/{book_id}/{cancelled_key}"),
            "cancelled.txt",
            &cancelled_hash,
            cancelled_text.len() as i64,
            &cancelled_fingerprint,
            1,
        )
        .await
        .unwrap();
    assert_eq!(
        repo.existing_chapter_import(
            actor,
            book_id,
            &cancelled_key,
            "cancelled.txt",
            &cancelled_hash,
            cancelled_text.len() as i64,
            &cancelled_fingerprint,
            1,
        )
        .await
        .unwrap(),
        Some(cancelled_job)
    );
    assert_eq!(
        repo.existing_chapter_import(
            actor,
            book_id,
            &cancelled_key,
            "cancelled.txt",
            &[9_u8; 32],
            cancelled_text.len() as i64,
            &cancelled_fingerprint,
            1,
        )
        .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        repo.enqueue_verified_chapter_import(
            stranger,
            book_id,
            &"d".repeat(43),
            &format!("staging/chapters/other-{nonce}"),
            "source.txt",
            &cancelled_hash,
            cancelled_text.len() as i64,
            &cancelled_fingerprint,
            1,
        )
        .await,
        Err(AuthorError::Missing)
    );
    assert!(repo
        .owned_chapter_import_status(stranger, book_id, cancelled_job)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.owned_chapter_import_status(actor, book_id, cancelled_job)
            .await
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );

    let (first, second) = tokio::join!(
        repo.claim_chapter_import(),
        repo.claim_chapter_import()
    );
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(
        claims.iter().filter(|claim| claim.is_some()).count(),
        1,
        "SKIP LOCKED excludes concurrent workers"
    );
    let old_claim = claims.into_iter().flatten().next().unwrap();
    assert_eq!(old_claim.job_id, cancelled_job);
    assert!(!repo
        .cancel_chapter_import(stranger, book_id, cancelled_job)
        .await
        .unwrap());
    assert!(repo
        .cancel_chapter_import(actor, book_id, cancelled_job)
        .await
        .unwrap());
    assert!(!repo
        .fail_chapter_import(cancelled_job, old_claim.epoch, false, "bad_source")
        .await
        .unwrap(), "cancel invalidates the old worker epoch");
    assert_eq!(
        repo.owned_chapter_import_status(actor, book_id, cancelled_job)
            .await
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
    assert!(repo
        .latest_active_chapter_import(actor, book_id)
        .await
        .unwrap()
        .is_none());

    // Expired lease is reclaimed with a new epoch; an old worker cannot commit.
    let text = "Глава 1: Первая\nТекст первой";
    let chapters = parse_text("source.txt", text).unwrap();
    let hash: [u8; 32] = Sha256::digest(text.as_bytes()).into();
    let approved = fingerprint(book_id, &chapters);
    let key = "e".repeat(43);
    let job = repo
        .enqueue_verified_chapter_import(
            actor,
            book_id,
            &key,
            &format!("staging/chapters/{actor}/{book_id}/{key}"),
            "source.txt",
            &hash,
            text.len() as i64,
            &approved,
            1,
        )
        .await
        .unwrap();
    let stale = repo.claim_chapter_import().await.unwrap().unwrap();
    assert_eq!(stale.job_id, job);
    sqlx::query(
        "UPDATE chapter_import_jobs SET locked_until=now()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(job)
    .execute(&pool)
    .await
    .unwrap();
    let current = repo.claim_chapter_import().await.unwrap().unwrap();
    assert_eq!(current.job_id, job);
    assert!(current.epoch > stale.epoch);
    assert_eq!(
        repo.complete_chapter_import_job(&stale, &chapters).await,
        Err(AuthorError::Conflict),
        "stale worker cannot finalize after lease recovery"
    );
    assert_eq!(
        repo.complete_chapter_import_job(&current, &chapters).await,
        Ok(1)
    );
    assert_eq!(
        repo.owned_chapter_import_status(actor, book_id, job)
            .await
            .unwrap()
            .unwrap()
            .status,
        "succeeded"
    );
    assert!(!repo
        .cancel_chapter_import(actor, book_id, job)
        .await
        .unwrap());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM chapters WHERE book_id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Race the cancel endpoint against the final transaction. Exactly one wins:
    // succeeded + one chapter, or cancelled + zero chapters from this batch.
    let race_text = "Глава 2: Вторая\nТекст второй";
    let race_chapters = parse_text("race.txt", race_text).unwrap();
    let race_hash: [u8; 32] = Sha256::digest(race_text.as_bytes()).into();
    let race_fingerprint = fingerprint(book_id, &race_chapters);
    let race_key = "f".repeat(43);
    let race_job = repo
        .enqueue_verified_chapter_import(
            actor,
            book_id,
            &race_key,
            &format!("staging/chapters/{actor}/{book_id}/{race_key}"),
            "race.txt",
            &race_hash,
            race_text.len() as i64,
            &race_fingerprint,
            1,
        )
        .await
        .unwrap();
    let race_claim = repo.claim_chapter_import().await.unwrap().unwrap();
    assert_eq!(race_claim.job_id, race_job);
    let (completed, cancelled) = tokio::join!(
        repo.complete_chapter_import_job(&race_claim, &race_chapters),
        repo.cancel_chapter_import(actor, book_id, race_job)
    );
    let final_status = repo
        .owned_chapter_import_status(actor, book_id, race_job)
        .await
        .unwrap()
        .unwrap()
        .status;
    let final_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapters WHERE book_id=$1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    match (completed, cancelled, final_status.as_str(), final_count) {
        (Ok(1), false, "succeeded", 2) => {}
        (Err(AuthorError::Conflict), true, "cancelled", 1) => {}
        other => panic!("cancel/finalize race was not atomic: {other:?}"),
    }
    pool.close().await;
}
