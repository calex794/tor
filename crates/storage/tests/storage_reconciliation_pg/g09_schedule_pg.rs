//! G-09 scheduler eligibility on disposable PostgreSQL only.
use super::*;
use tor_storage::repositories::reconciliation_schedule::{
    PgG09Schedule, ScheduledAuditAction as Action,
};

const SCHEDULE_BUCKET: &str = "tor-private-g09-schedule-ci";

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn g09_scheduled_scan_respects_global_checkpoint_cadence_and_exhaustion() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never access production PG");
    let pool = PgPool::connect(&url).await.unwrap();
    let schedule = PgG09Schedule::new(pool.clone());
    schedule.probe().await.unwrap();

    // Absence of a checkpoint allows a first page only after explicit worker opt-in.
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, false).await.unwrap(),
        Action::ScanPage
    );
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, true).await.unwrap(),
        Action::ScanPage
    );

    sqlx::query(
        "INSERT INTO g09_private_inventory_scans
            (bucket,scope,completed,checkpoint_at)
         VALUES($1,'chapter',TRUE,clock_timestamp())",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool).await.unwrap();
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, false).await.unwrap(),
        Action::Wait,
        "a completed cycle cannot restart immediately"
    );
    sqlx::query(
        "UPDATE g09_private_inventory_scans
            SET checkpoint_at=clock_timestamp()-interval '25 hours'
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool).await.unwrap();
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, false).await.unwrap(),
        Action::NewCycle
    );
    sqlx::query(
        "UPDATE g09_private_inventory_scans
            SET completed=FALSE,checkpoint_at=clock_timestamp()
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool).await.unwrap();
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, false).await.unwrap(),
        Action::Wait
    );
    sqlx::query(
        "UPDATE g09_private_inventory_scans
            SET checkpoint_at=clock_timestamp()-interval '16 minutes'
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool).await.unwrap();
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, false).await.unwrap(),
        Action::ScanPage
    );

    sqlx::query(
        "INSERT INTO g09_registered_object_scans
            (bucket,scope,exhausted,completed,checkpoint_at)
         VALUES($1,'chapter',TRUE,FALSE,clock_timestamp()-interval '25 hours')",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool).await.unwrap();
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, true).await.unwrap(),
        Action::Wait,
        "scheduler must never bypass the eight-S3-error operator gate"
    );
    sqlx::query(
        "UPDATE g09_registered_object_scans
            SET exhausted=FALSE
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool).await.unwrap();
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, true).await.unwrap(),
        Action::ScanPage
    );
    sqlx::query(
        "UPDATE g09_registered_object_scans
            SET completed=TRUE,checkpoint_at=clock_timestamp()
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool).await.unwrap();
    assert_eq!(
        schedule.action(SCHEDULE_BUCKET, PrivateScope::Chapter, true).await.unwrap(),
        Action::Wait
    );

    // G-owned test rows only, never source storage objects.
    sqlx::query("DELETE FROM g09_registered_object_scans WHERE bucket=$1")
        .bind(SCHEDULE_BUCKET).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM g09_private_inventory_scans WHERE bucket=$1")
        .bind(SCHEDULE_BUCKET).execute(&pool).await.unwrap();
    pool.close().await;
}
