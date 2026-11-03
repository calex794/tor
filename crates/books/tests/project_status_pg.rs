//! S020: owner status updates cannot publish books or revoke purchases.
use sqlx::PgPool;
use tor_books::repositories::postgres::PgAuthoring;
use tor_books::services::authoring::AuthorError;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_owner_status_cas_is_private_and_independent_of_visibility() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 owner','$argon2id$test') RETURNING id"
    )
    .bind(format!("s020-project-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 other','$argon2id$test') RETURNING id"
    )
    .bind(format!("s020-other-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           project_status,visibility)
         VALUES($1,'ru',$2,$3,'ongoing','draft') RETURNING id"
    )
    .bind(format!("s020:status:{marker}"))
    .bind(format!("Состояние книги {marker}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let author = PgAuthoring::new(pool.clone());
    let first = author.owned_project_status(owner, book_id).await.unwrap().unwrap();
    assert_eq!(first.status, "ongoing");
    assert_eq!(first.revision, 1);
    assert!(author.owned_project_status(outsider, book_id).await.unwrap().is_none());
    assert_eq!(
        author.update_project_status(outsider, book_id, 1, "paused").await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        author.update_project_status(owner, book_id, 1, "public").await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        author.update_project_status(owner, book_id, 1, "paused").await,
        Ok(2)
    );
    assert_eq!(
        author.update_project_status(owner, book_id, 1, "completed").await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        author.update_project_status(owner, book_id, 2, "completed").await,
        Ok(3)
    );
    let (state, revision, visibility, access): (String, i64, String, String) =
        sqlx::query_as(
            "SELECT project_status,project_status_revision,visibility,access_level
             FROM books WHERE id=$1"
        )
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "completed");
    assert_eq!(revision, 3);
    assert_eq!(visibility, "draft");
    assert_eq!(access, "basic");
    pool.close().await;
}
