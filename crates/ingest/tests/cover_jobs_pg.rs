//! Real PostgreSQL G-07 lease and ownership tests. CI injects isolated fixtures.
use sqlx::postgres::PgPoolOptions;
use tor_ingest::{
    domain::cover::CoverError,
    repositories::cover_jobs::PgCoverJobs,
    services::{cover_jobs::CoverJobs, image_pipeline::RECIPE_VERSION},
};

#[tokio::test]
#[ignore = "requires TOR PG18 test service and seeded owner books"]
async fn g_07_claim_fence_recovery_and_forbidden_registration() {
    let url = std::env::var("TEST_DATABASE_URL").expect("CI PG URL");
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let repo = PgCoverJobs::new(pool.clone());
    assert_eq!(
        repo.register_verified_source(
            905,
            901,
            "staging/covers/no-foreign-owner.png",
            "image/png",
            10,
            &[1_u8; 32]
        )
        .await
        .unwrap_err(),
        CoverError::Forbidden
    );
    let source = repo
        .register_verified_source(
            905,
            905,
            "staging/covers/pg-fencing-source-905.png",
            "image/png",
            10,
            &[1_u8; 32],
        )
        .await
        .unwrap();
    let job = repo
        .enqueue(source, 905, 905, 0, RECIPE_VERSION)
        .await
        .unwrap();
    assert_eq!(
        job,
        repo.enqueue(source, 905, 905, 0, RECIPE_VERSION)
            .await
            .unwrap()
    );
    assert_eq!(
        repo.job_status(job).await.unwrap().as_deref(),
        Some("pending")
    );
    // Operator counters reflect the actual job state, without exposing an actor.
    assert!(repo.queue_counts().await.unwrap().pending >= 1);
    sqlx::query(
        "UPDATE cover_variant_jobs SET created_at=now()-INTERVAL '10 minutes'
         WHERE id=$1",
    )
    .bind(job)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        repo.queue_counts().await.unwrap().overdue_pending >= 1,
        "runnable old jobs must be visible without exposing owner IDs"
    );
    sqlx::query("UPDATE cover_variant_jobs SET created_at=now() WHERE id=$1")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();


    let (one, two) = tokio::join!(repo.claim_next(), repo.claim_next());
    let claims: Vec<_> = [one.unwrap(), two.unwrap()].into_iter().flatten().collect();
    assert_eq!(
        claims.len(),
        1,
        "a job must not be claimed by both concurrent workers"
    );
    let old = &claims[0];
    assert_eq!(old.job_id, job);
    assert_eq!(old.epoch, 1);
    // Simulate a dead worker and its lease becoming claimable.
    sqlx::query("UPDATE cover_variant_jobs SET locked_until=now()-INTERVAL '1 second' WHERE id=$1")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        repo.queue_counts().await.unwrap().expired_lease >= 1,
        "an expired cover lease is visible to the operator"
    );
    let latest = repo.claim_next().await.unwrap().unwrap();
    assert_eq!(latest.job_id, job);
    assert_eq!(latest.epoch, old.epoch + 1);
    assert_eq!(
        repo.record_failure(job, old.epoch, false, "stale_worker")
            .await
            .unwrap_err(),
        CoverError::LostLease
    );
    assert_eq!(
        repo.job_status(job).await.unwrap().as_deref(),
        Some("running")
    );
    repo.record_failure(job, latest.epoch, false, "negative_fixture")
        .await
        .unwrap();
    assert_eq!(repo.job_status(job).await.unwrap().as_deref(), Some("dead"));
    sqlx::query("UPDATE cover_variant_jobs SET attempts=3 WHERE id=$1")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();
    let signals = repo.queue_counts().await.unwrap();
    assert!(signals.dead >= 1);
    assert!(signals.repeated_attempts >= 1);

    pool.close().await;
}
