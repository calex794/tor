//! S023 G repository: PostgreSQL lease, retry, idempotency and owner-scope regression.
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_imports::{
    repositories::postgres::PgImportJobs,
    domain::ImportJobsError,
};

async fn insert_user(pool: &PgPool, email: String, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,$2,'$argon2id$test') RETURNING id",
    )
    .bind(email)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_draft_book(pool: &PgPool, actor: i64, identity: String) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books
         (source_identity,target_language,title,visibility,owner_user_id,
          author_name,genres,tags,fandoms,description,access_level)
         VALUES($1,'ru','Queue fixture','draft',$2,'',
                ARRAY[]::text[],ARRAY[]::text[],ARRAY[]::text[],'','basic')
         RETURNING id",
    )
    .bind(identity)
    .bind(actor)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn s023_queue_claim_fencing_retry_and_owner_privacy() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "no production database");
    let pool = PgPool::connect(&url).await.unwrap();
    let jobs = PgImportJobs::new(pool.clone());
    jobs.probe().await.unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor = insert_user(
        &pool,
        format!("s023-g-owner-{nonce}@tor-ci.example"),
        "Queue owner",
    )
    .await;
    let stranger = insert_user(
        &pool,
        format!("s023-g-stranger-{nonce}@tor-ci.example"),
        "Stranger",
    )
    .await;
    let book = insert_draft_book(&pool, actor, format!("tor-ci:s023-g:{nonce}")).await;

    let command = "c".repeat(43);
    let key = format!("staging/chapters/{actor}/{book}/{command}");
    let hash = [3_u8; 32];
    let approved = [4_u8; 32];
    let job = jobs
        .enqueue_verified(
            actor,
            book,
            &command,
            &key,
            "source.txt",
            &hash,
            100,
            &approved,
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        jobs.enqueue_verified(
            actor,
            book,
            &command,
            &key,
            "source.txt",
            &hash,
            100,
            &approved,
            2,
        )
        .await
        .unwrap(),
        job,
        "same command key and payload are idempotent"
    );
    assert_eq!(
        jobs.existing_for_command(
            actor,
            book,
            &command,
            "source.txt",
            &[9_u8; 32],
            100,
            &approved,
            2,
        )
        .await,
        Err(ImportJobsError::Conflict)
    );
    assert_eq!(
        jobs.enqueue_verified(
            stranger,
            book,
            &"d".repeat(43),
            &format!("staging/chapters/{stranger}/{book}/stranger"),
            "source.txt",
            &hash,
            100,
            &approved,
            2,
        )
        .await,
        Err(ImportJobsError::Missing)
    );
    assert!(jobs.status_for_owner(stranger, book, job).await.unwrap().is_none());

    // A different idempotency key cannot create a second active job for the book.
    assert_eq!(
        jobs.enqueue_verified(
            actor,
            book,
            &"e".repeat(43),
            &format!("staging/chapters/{actor}/{book}/second"),
            "source.txt",
            &hash,
            100,
            &approved,
            2,
        )
        .await,
        Err(ImportJobsError::Conflict)
    );

    let (first, second) = tokio::join!(jobs.claim_next(), jobs.claim_next());
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(
        claims.iter().filter(|claim| claim.is_some()).count(),
        1,
        "SKIP LOCKED gives a job to only one worker"
    );
    let old = claims.into_iter().flatten().next().unwrap();
    assert_eq!(old.job_id, job);
    assert!(!jobs
        .cancel_source_for_owner(stranger, book, job)
        .await
        .unwrap()
        .is_some());
    assert!(jobs
        .cancel_source_for_owner(actor, book, job)
        .await
        .unwrap()
        .is_some());
    assert!(!jobs
        .fail_if_current(job, old.epoch, false, "stale_worker")
        .await
        .unwrap());
    assert_eq!(
        jobs.status_for_owner(actor, book, job)
            .await
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
    assert!(jobs
        .latest_active_for_owner(actor, book)
        .await
        .unwrap()
        .is_none());

    // Expired lease is recovered with a larger epoch; stale failure cannot
    // overwrite the replacement worker's state.
    let command2 = "f".repeat(43);
    let key2 = format!("staging/chapters/{actor}/{book}/{command2}");
    let job2 = jobs
        .enqueue_verified(
            actor,
            book,
            &command2,
            &key2,
            "source.md",
            &[5_u8; 32],
            100,
            &[6_u8; 32],
            1,
        )
        .await
        .unwrap();
    let stale = jobs.claim_next().await.unwrap().unwrap();
    assert_eq!(stale.job_id, job2);
    sqlx::query(
        "UPDATE chapter_import_jobs SET locked_until=now()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(job2)
    .execute(&pool)
    .await
    .unwrap();
    let current = jobs.claim_next().await.unwrap().unwrap();
    assert_eq!(current.job_id, job2);
    assert!(current.epoch > stale.epoch);
    assert!(!jobs
        .fail_if_current(job2, stale.epoch, false, "old_worker")
        .await
        .unwrap());
    assert!(jobs
        .fail_if_current(job2, current.epoch, true, "temporary_store_error")
        .await
        .unwrap());
    assert_eq!(
        jobs.status_for_owner(actor, book, job2)
            .await
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );

    // Exhausted attempts are moved to failed rather than stranded as running.
    sqlx::query(
        "UPDATE chapter_import_jobs SET status='running',attempts=8,
             locked_until=now()-INTERVAL '1 second' WHERE id=$1 AND status='queued'",
    )
    .bind(job2)
    .execute(&pool)
    .await
    .unwrap();
    let _ = jobs.claim_next().await.unwrap();
    assert_eq!(
        jobs.status_for_owner(actor, book, job2)
            .await
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
    pool.close().await;
}
