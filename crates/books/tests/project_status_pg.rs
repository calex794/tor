//! S020: scoped member status updates cannot publish books or revoke purchases.
use sqlx::PgPool;
use tor_books::repositories::{book_members::PgBookMembers, postgres::PgAuthoring};
use tor_books::services::authoring::AuthorError;
use tor_contracts::access::BookRole;

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
         VALUES($1,'S020 owner','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-project-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 other','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-other-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let translator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 translator','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-translator-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let moderator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 moderator','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-moderator-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           project_status,visibility,access_level)
         VALUES($1,'ru',$2,$3,'ongoing','draft','basic') RETURNING id",
    )
    .bind(format!("s020:status:{marker}"))
    .bind(format!("Состояние книги {marker}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let members = PgBookMembers::new(pool.clone());
    members
        .add_collaborator(owner, book_id, translator, BookRole::Translator)
        .await
        .unwrap();
    members
        .add_collaborator(owner, book_id, moderator, BookRole::Moderator)
        .await
        .unwrap();
    let foreign_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id)
         VALUES($1,'ru',$2,$3) RETURNING id",
    )
    .bind(format!("s020:unrelated:{marker}"))
    .bind(format!("Unrelated {marker}"))
    .bind(outsider)
    .fetch_one(&pool)
    .await
    .unwrap();
    let author = PgAuthoring::new(pool.clone());
    let first = author
        .manageable_project_status(owner, book_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.status, "ongoing");
    assert_eq!(first.revision, 1);
    assert!(author
        .manageable_project_status(outsider, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        author
            .update_project_status(outsider, book_id, 1, "paused")
            .await,
        Err(AuthorError::Missing)
    );
    assert!(author
        .manageable_project_status(translator, book_id)
        .await
        .unwrap()
        .is_some());
    assert!(author
        .manageable_project_status(moderator, book_id)
        .await
        .unwrap()
        .is_some());
    assert!(author
        .manageable_project_status(translator, foreign_book)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        author
            .update_project_status(translator, foreign_book, 1, "paused")
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        author
            .update_project_status(owner, book_id, 1, "public")
            .await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        author
            .update_project_status(owner, book_id, 1, "paused")
            .await,
        Ok(2)
    );
    assert_eq!(
        author
            .update_project_status(owner, book_id, 1, "completed")
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        author
            .update_project_status(owner, book_id, 2, "completed")
            .await,
        Ok(3)
    );
    assert_eq!(
        author
            .update_project_status(translator, book_id, 2, "paused")
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        author
            .update_project_status(translator, book_id, 3, "paused")
            .await,
        Ok(4)
    );
    assert_eq!(
        author
            .update_project_status(moderator, book_id, 4, "planned")
            .await,
        Ok(5)
    );
    members
        .revoke_collaborator(owner, book_id, translator)
        .await
        .unwrap();
    assert!(author
        .manageable_project_status(translator, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        author
            .update_project_status(translator, book_id, 5, "completed")
            .await,
        Err(AuthorError::Missing)
    );
    let (state, revision, visibility, access): (String, i64, String, String) = sqlx::query_as(
        "SELECT project_status,project_status_revision,visibility,access_level
             FROM books WHERE id=$1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "planned");
    assert_eq!(revision, 5);
    assert_eq!(visibility, "draft");
    assert_eq!(access, "basic");
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(author
        .manageable_project_status(moderator, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        author
            .update_project_status(moderator, book_id, 5, "completed")
            .await,
        Err(AuthorError::Missing)
    );
    pool.close().await;
}
