//! S006/R1: failed PostgreSQL progress saves must never acknowledge a new revision.
use super::*;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci PostgreSQL"]
async fn c_09_closed_pool_preserves_last_acknowledged_progress_and_recovers() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG test database");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    let witness = sqlx::PgPool::connect(&url).await.unwrap();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('c09-outage-progress@example.invalid','Progress outage','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&witness)
    .await
    .unwrap();
    let saved = ReadingPlace {
        block_index: 1,
        char_offset: 1,
    };
    let later = ReadingPlace {
        block_index: 1,
        char_offset: 2,
    };
    let offline = PgReadingSync::new(pool.clone());
    let first = offline.save_free(user, 101, 201, saved, 0).await.unwrap();
    assert_eq!(first.revision, 1);
    pool.close().await;

    let failure = offline.save_free(user, 101, 201, later, 1).await;
    assert_eq!(failure, Err(SyncError::Unavailable));
    assert_eq!(
        offline.free_state(user, 101).await,
        Err(SyncError::Unavailable)
    );

    // Query through an independent live connection. A failed save is not a
    // successful revision and cannot erase the last durable reader cursor.
    let recovered = PgReadingSync::new(witness.clone());
    let unchanged = recovered.free_state(user, 101).await.unwrap().unwrap();
    assert_eq!(unchanged.receipt.revision, 1);
    assert_eq!(unchanged.place, saved);

    let second = recovered.save_free(user, 101, 201, later, 1).await.unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(
        recovered.save_free(user, 101, 201, later, 1).await,
        Err(SyncError::Conflict),
        "a lost response must not turn an old revision into a second commit"
    );
    let final_state = recovered.free_state(user, 101).await.unwrap().unwrap();
    assert_eq!(final_state.receipt.revision, 2);
    assert_eq!(final_state.place, later);

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reader_book_positions WHERE user_id=$1")
            .bind(user)
            .fetch_one(&witness)
            .await
            .unwrap();
    assert_eq!(count, 1);
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&witness)
        .await
        .unwrap();
    witness.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL PostgreSQL 18"]
async fn c_09_deferred_progress_commit_rejection_keeps_last_receipt_and_recovers() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert_eq!(
        url, "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci",
        "COMMIT fault injection may never run against production"
    );
    let migrator_url = std::env::var("MIGRATOR_URL").expect("disposable migrator");
    assert_eq!(
        migrator_url,
        "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci"
    );
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email = format!("c09-deferred-{nonce}@example.invalid");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'COMMIT progress reader','$argon2id$fixture')
         RETURNING id",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap();
    let sync = PgReadingSync::new(pool.clone());
    let acknowledged = ReadingPlace {
        block_index: 1,
        char_offset: 1,
    };
    let next = ReadingPlace {
        block_index: 1,
        char_offset: 2,
    };
    let first = sync
        .save_free(user, 101, 201, acknowledged, 0)
        .await
        .unwrap();
    assert_eq!(first.revision, 1);
    let before = sync.free_state(user, 101).await.unwrap().unwrap();

    // The row UPDATE succeeds inside the transaction but its initially
    // deferred constraint trigger rejects COMMIT. No new revision may be
    // exposed as a successful progress receipt or lost from the old cursor.
    let migrator = sqlx::PgPool::connect(&migrator_url).await.unwrap();
    sqlx::query(
        "CREATE FUNCTION ci_c09_reject_progress_commit() RETURNS trigger AS
         'BEGIN
            IF EXISTS (
              SELECT 1 FROM users WHERE id=NEW.user_id
                AND email_normalized LIKE ''c09-deferred-%@example.invalid''
            ) THEN
              RAISE EXCEPTION ''C09 injected progress COMMIT rollback''
                USING ERRCODE = ''40001'';
            END IF;
            RETURN NULL;
          END'
         LANGUAGE plpgsql",
    )
    .execute(&migrator)
    .await
    .unwrap();
    sqlx::query(
        "CREATE CONSTRAINT TRIGGER ci_c09_deferred_progress_commit
         AFTER UPDATE ON reader_book_positions
         DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW EXECUTE FUNCTION ci_c09_reject_progress_commit()",
    )
    .execute(&migrator)
    .await
    .unwrap();

    assert_eq!(
        sync.save_free(user, 101, 201, next, 1).await,
        Err(SyncError::Unavailable),
        "COMMIT error must not acknowledge the second cursor"
    );
    let durable = PgReadingSync::new(pool.clone())
        .free_state(user, 101)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.receipt.revision, first.revision);
    assert_eq!(durable.receipt.updated_at, before.receipt.updated_at);
    assert_eq!(durable.place, acknowledged);
    let positions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_book_positions WHERE user_id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(positions, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM reader_chapter_completions WHERE user_id=$1"
        )
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
        "an aborted cursor update must not mint completion"
    );

    sqlx::query("DROP TRIGGER ci_c09_deferred_progress_commit ON reader_book_positions")
        .execute(&migrator)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION ci_c09_reject_progress_commit()")
        .execute(&migrator)
        .await
        .unwrap();
    migrator.close().await;

    // Same expected_revision=1 is still valid after a rejected COMMIT.
    let recovery = sync.save_free(user, 101, 201, next, 1).await.unwrap();
    assert_eq!(recovery.revision, 2);
    assert_eq!(
        sync.save_free(user, 101, 201, next, 1).await,
        Err(SyncError::Conflict)
    );
    let final_state = sync.free_state(user, 101).await.unwrap().unwrap();
    assert_eq!(final_state.receipt.revision, 2);
    assert_eq!(final_state.place, next);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM reader_book_positions WHERE user_id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
