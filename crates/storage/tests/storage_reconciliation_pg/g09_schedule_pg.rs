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
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, false)
            .await
            .unwrap(),
        Action::ScanPage
    );
    assert_eq!(
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, true)
            .await
            .unwrap(),
        Action::ScanPage
    );
    // Two TOR replicas race for one global time-slot. Exactly one wins,
    // even though both observed a due checkpoint.
    let competitor = PgG09Schedule::new(pool.clone());
    let (a, b) = tokio::join!(
        schedule.reserve_slot(SCHEDULE_BUCKET, PrivateScope::Chapter, false),
        competitor.reserve_slot(SCHEDULE_BUCKET, PrivateScope::Chapter, false),
    );
    let claimed = [a.unwrap(), b.unwrap()]
        .into_iter()
        .filter(|value| *value)
        .count();
    assert_eq!(claimed, 1, "one global LIST slot per 15 minutes");
    assert!(!schedule
        .reserve_slot(SCHEDULE_BUCKET, PrivateScope::Chapter, false)
        .await
        .unwrap());
    // A separate direction is independently quota-controlled.
    assert!(competitor
        .reserve_slot(SCHEDULE_BUCKET, PrivateScope::Chapter, true)
        .await
        .unwrap());
    assert!(!schedule
        .reserve_slot(SCHEDULE_BUCKET, PrivateScope::Chapter, true)
        .await
        .unwrap());
    sqlx::query(
        "UPDATE g09_scheduled_pages
            SET next_allowed_at=clock_timestamp()-interval '1 second'
          WHERE bucket=$1 AND direction='s3-to-pg'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert!(schedule
        .reserve_slot(SCHEDULE_BUCKET, PrivateScope::Chapter, false)
        .await
        .unwrap());

    sqlx::query(
        "INSERT INTO g09_private_inventory_scans
            (bucket,scope,completed,checkpoint_at)
         VALUES($1,'chapter',TRUE,clock_timestamp())",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, false)
            .await
            .unwrap(),
        Action::Wait,
        "a completed cycle cannot restart immediately"
    );
    sqlx::query(
        "UPDATE g09_private_inventory_scans
            SET checkpoint_at=clock_timestamp()-interval '25 hours'
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, false)
            .await
            .unwrap(),
        Action::NewCycle
    );
    sqlx::query(
        "UPDATE g09_private_inventory_scans
            SET completed=FALSE,checkpoint_at=clock_timestamp()
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, false)
            .await
            .unwrap(),
        Action::Wait
    );
    sqlx::query(
        "UPDATE g09_private_inventory_scans
            SET checkpoint_at=clock_timestamp()-interval '16 minutes'
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, false)
            .await
            .unwrap(),
        Action::ScanPage
    );

    sqlx::query(
        "INSERT INTO g09_registered_object_scans
            (bucket,scope,exhausted,completed,checkpoint_at)
         VALUES($1,'chapter',TRUE,FALSE,clock_timestamp()-interval '25 hours')",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, true)
            .await
            .unwrap(),
        Action::Wait,
        "scheduler must never bypass the eight-S3-error operator gate"
    );
    sqlx::query(
        "UPDATE g09_registered_object_scans
            SET exhausted=FALSE
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, true)
            .await
            .unwrap(),
        Action::ScanPage
    );
    sqlx::query(
        "UPDATE g09_registered_object_scans
            SET completed=TRUE,checkpoint_at=clock_timestamp()
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(SCHEDULE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        schedule
            .action(SCHEDULE_BUCKET, PrivateScope::Chapter, true)
            .await
            .unwrap(),
        Action::Wait
    );

    // G-owned test rows only, never source storage objects.
    sqlx::query("DELETE FROM g09_scheduled_pages WHERE bucket=$1")
        .bind(SCHEDULE_BUCKET)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM g09_registered_object_scans WHERE bucket=$1")
        .bind(SCHEDULE_BUCKET)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM g09_private_inventory_scans WHERE bucket=$1")
        .bind(SCHEDULE_BUCKET)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

const FAILING_BUCKET: &str = "tor-private-g09-list-errors-ci";

struct FailingList {
    unavailable: std::sync::atomic::AtomicBool,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl PrivateInventoryLister for FailingList {
    fn private_bucket(&self) -> &str {
        FAILING_BUCKET
    }

    async fn list_private_page(
        &self,
        _scope: PrivateScope,
        _continuation: Option<&str>,
    ) -> Result<PrivateInventoryPage, StorageError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            Err(StorageError::Unavailable)
        } else {
            Ok(PrivateInventoryPage {
                objects: Vec::new(),
                continuation: None,
            })
        }
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 /tor_ci"]
async fn g09_forward_list_stops_after_eight_errors_until_operator_resume() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never touch durable PG");
    let pool = PgPool::connect(&url).await.unwrap();
    let forward = PgPrivateInventory::new(pool.clone());
    let scheduler = PgG09Schedule::new(pool.clone());
    let store = FailingList {
        unavailable: std::sync::atomic::AtomicBool::new(true),
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    for _ in 0..8 {
        assert_eq!(
            forward
                .scan_page(&store, PrivateScope::Chapter)
                .await
                .unwrap_err(),
            InventoryError::Unavailable
        );
    }
    let stopped = forward
        .progress(FAILING_BUCKET, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert!(stopped.exhausted);
    assert!(!stopped.completed);
    assert_eq!(
        (stopped.scanned, stopped.unknown_attempts, stopped.retries),
        (0, 8, 8)
    );
    assert_eq!(
        scheduler
            .action(FAILING_BUCKET, PrivateScope::Chapter, false)
            .await
            .unwrap(),
        Action::Wait,
        "no automatic scheduler retry after eight LIST provider errors"
    );
    assert!(forward
        .scan_page(&store, PrivateScope::Chapter)
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.calls.load(Ordering::SeqCst), 8);
    store.unavailable.store(false, Ordering::SeqCst);
    assert!(forward
        .resume_exhausted(FAILING_BUCKET, PrivateScope::Chapter)
        .await
        .unwrap());
    let done = forward
        .scan_page(&store, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert!(done.completed);
    assert!(!done.exhausted);
    assert_eq!(done.unknown_attempts, 8);
    assert_eq!(store.calls.load(Ordering::SeqCst), 9);
    sqlx::query("DELETE FROM g09_private_inventory_scans WHERE bucket=$1")
        .bind(FAILING_BUCKET)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
