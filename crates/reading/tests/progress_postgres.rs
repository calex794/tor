//! PostgreSQL 18 C-04 contract: progress never grants access to locked books.
use tor_reading::{
    repositories::PgReadingProgress,
    services::progress::{ProgressError, ReadingPlace},
};

#[tokio::test]
#[ignore = "requires disposable tor_ci database with published chapter fixtures"]
async fn c_04_resume_is_private_monotonic_and_current_version_only() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    assert!(url.ends_with("/tor_ci"), "never modify a production database");
    let pool = sqlx::PgPool::connect(&url).await.expect("postgres");
    let repo = PgReadingProgress::new(pool.clone());
    let user_one: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES ('progress.one@example.invalid','Reader One','test-hash') RETURNING id",
    ).fetch_one(&pool).await.unwrap();
    let user_two: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES ('progress.two@example.invalid','Reader Two','test-hash') RETURNING id",
    ).fetch_one(&pool).await.unwrap();

    let first = ReadingPlace { block_index: 1, char_offset: 2 };
    let later = ReadingPlace { block_index: 2, char_offset: 9 };
    repo.save_position(user_one, 101, 201, first).await.unwrap();
    repo.save_position(user_one, 101, 201, later).await.unwrap();
    repo.save_position(user_one, 101, 201, first).await.unwrap();
    let saved = repo.saved_place(user_one, 101, 201).await.unwrap().unwrap();
    assert_eq!(saved, later, "delayed request must not move progress backwards");
    assert_eq!(repo.saved_place(user_two, 101, 201).await.unwrap(), None);
    assert!(repo.continue_reading(user_two).await.unwrap().is_empty());
    let reading = repo.continue_reading(user_one).await.unwrap();
    assert_eq!(reading.len(), 1);
    assert_eq!(reading[0].chapter_id, 101);
    assert_eq!(reading[0].block_index, 2);

    // No payment or read rights can be inferred from a stored position.
    for denied in [102,103,104,105,106,107] {
        assert_eq!(
            repo.save_position(user_one, denied, 201 + denied - 101, first).await,
            Err(ProgressError::NotFound),
            "cannot save cursor into a protected chapter: {denied}"
        );
    }
    assert_eq!(
        repo.save_position(user_one, 101, 999, first).await,
        Err(ProgressError::StaleVersion)
    );
    assert_eq!(
        repo.save_position(user_one, 101, 201, ReadingPlace {block_index: 0,char_offset: 0}).await,
        Err(ProgressError::Invalid)
    );
    pool.close().await;
}
