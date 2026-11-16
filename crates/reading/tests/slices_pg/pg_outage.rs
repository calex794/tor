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
    assert_eq!(offline.free_state(user, 101).await, Err(SyncError::Unavailable));

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

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reader_book_positions WHERE user_id=$1",
    )
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
