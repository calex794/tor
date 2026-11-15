//! Concurrency, independent cleanup and real paid C authority.
use super::Fixture;
use tor_reading::services::{progress::ReadingPlace, sync::SyncError};

#[tokio::test]
#[ignore = "requires migrated disposable tor_ci PostgreSQL"]
async fn heartbeat_does_not_block_reading_and_sealing_fences_disable_capture() {
    let mut f = Fixture::new().await;
    let user = f.user().await;
    let book = f.book().await;
    f.start().await;
    f.enroll(user, "standard").await;
    sqlx::query("UPDATE surprise_experiment_participants SET enrolled_at=now()-interval '25 hours' WHERE run_id=$1")
        .bind(f.run).execute(&f.pool).await.unwrap();
    let mut heartbeat = f.pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE surprise_experiment_runs SET last_checked_at=clock_timestamp() WHERE id=$1",
    )
    .bind(f.run)
    .execute(&mut *heartbeat)
    .await
    .unwrap();
    f.advance(user, book, 1, 1, 0).await;
    assert!(f.bits(user).await.2, "heartbeat must not lose D1 capture");
    let healthy: bool = sqlx::query_scalar(
        "SELECT faults_start=surprise_capture_faults() FROM surprise_experiment_runs WHERE id=$1",
    )
    .bind(f.run)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(
        healthy,
        "ordinary heartbeat cannot invalidate a valid trial"
    );
    heartbeat.rollback().await.unwrap();

    // An exclusive close fence blocks optional capture, but must not prevent
    // the user's disable choice. Lost measurement invalidates the trial.
    let mut closing = f.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM surprise_experiment_runs WHERE id=$1 FOR UPDATE")
        .bind(f.run)
        .execute(&mut *closing)
        .await
        .unwrap();
    f.surprises.disable(user).await.unwrap();
    closing.rollback().await.unwrap();
    let disabled: bool =
        sqlx::query_scalar("SELECT disabled FROM surprise_user_preferences WHERE user_id=$1")
            .bind(user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(disabled);
    f.surprises.maintain_experiment().await.unwrap();
    let report = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.decision, "capture_failure");
    assert_eq!(report.state, "stopped");
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable tor_ci PostgreSQL"]
async fn aggregate_contention_does_not_prevent_fail_closed_retention() {
    let mut f = Fixture::new().await;
    let user = f.user().await;
    f.start().await;
    f.enroll(user, "standard").await;
    sqlx::query("UPDATE surprise_experiment_participants SET enrolled_at=now()-interval '731 hours' WHERE run_id=$1")
        .bind(f.run).execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE surprise_experiment_runs SET last_checked_at=now()-interval '3 minutes' WHERE id=$1")
        .bind(f.run).execute(&f.pool).await.unwrap();
    let mut blocked_report = f.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT run_id FROM surprise_experiment_totals WHERE run_id=$1 AND arm='control' FOR UPDATE",
    )
    .bind(f.run)
    .execute(&mut *blocked_report)
    .await
    .unwrap();
    assert!(f.surprises.maintain_experiment().await.is_err());
    f.surprises.recover_expired_experiment().await.unwrap();
    let state: (String, String) =
        sqlx::query_as("SELECT state,decision FROM surprise_experiment_runs WHERE id=$1")
            .bind(f.run)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(state, ("stopped".into(), "monitor_gap".into()));
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM surprise_experiment_participants WHERE run_id=$1")
            .bind(f.run)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        receipts, 0,
        "a broken report cannot retain expired identities"
    );
    let flags: (bool, bool) = sqlx::query_as(
        "SELECT enabled,experiment_enabled FROM surprise_runtime_policy WHERE singleton",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(flags, (false, false));
    blocked_report.rollback().await.unwrap();
    let report = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert!(report.differences.is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable tor_ci PostgreSQL"]
async fn paid_pinned_v1_is_measured_but_revoked_and_foreign_purchases_are_not() {
    let mut f = Fixture::new().await;
    let buyer = f.user().await;
    let refunded = f.user().await;
    let book = f.book().await;
    sqlx::query("UPDATE chapters SET access_level='paid',price_amount=100 WHERE id=$1")
        .bind(book.chapter)
        .execute(&f.pool)
        .await
        .unwrap();
    let mut purchases = Vec::<i64>::new();
    for user in [buyer, refunded] {
        let purchase = sqlx::query_scalar(
            "INSERT INTO chapter_purchase_facts
               (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
             VALUES($1,$2,$3,$4,100,'RUB') RETURNING id",
        )
        .bind(user)
        .bind(book.chapter)
        .bind(book.version)
        .bind(format!("{}-purchase-{user}", f.prefix))
        .fetch_one(&f.pool)
        .await
        .unwrap();
        purchases.push(purchase);
    }
    f.start().await;
    f.enroll(buyer, "standard").await;
    f.enroll(refunded, "control").await;
    let early = ReadingPlace {
        block_index: 1,
        char_offset: 1,
    };
    f.reading
        .save_purchased(buyer, purchases[0], book.chapter, book.version, early, 0)
        .await
        .unwrap();
    assert!(!f.bits(buyer).await.0);
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Another public edition') RETURNING id",
    )
    .bind(book.chapter)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(book.chapter)
        .bind(v2)
        .execute(&f.pool)
        .await
        .unwrap();
    let last = ReadingPlace {
        block_index: 2,
        char_offset: 8,
    };
    assert_eq!(
        f.reading
            .save_purchased(refunded, purchases[0], book.chapter, book.version, last, 0)
            .await,
        Err(SyncError::NotFound)
    );
    f.reading
        .save_purchased(buyer, purchases[0], book.chapter, book.version, last, 1)
        .await
        .unwrap();
    assert!(
        f.bits(buyer).await.0,
        "pinned V1 completion is canonical reading"
    );
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,$2,'refund')",
    )
    .bind(purchases[1])
    .bind(format!("{}-refund", f.prefix))
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        f.reading
            .save_purchased(refunded, purchases[1], book.chapter, book.version, last, 0)
            .await,
        Err(SyncError::NotFound)
    );
    assert!(!f.bits(refunded).await.0);
    let healthy: bool = sqlx::query_scalar(
        "SELECT faults_start=surprise_capture_faults() FROM surprise_experiment_runs WHERE id=$1",
    )
    .bind(f.run)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(healthy);
    // D facts are immutable. Keep their isolated actors/editions until the
    // disposable database is dropped; never disable financial protections.
    f.users.retain(|id| ![buyer, refunded].contains(id));
    f.books.retain(|item| item.id != book.id);
    f.cleanup().await;
}
