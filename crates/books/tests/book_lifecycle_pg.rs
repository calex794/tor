//! S020: NEwBOO lifecycle remains separate from publication and project status.
//! Active Book members may edit only safe values; deletion requires another flow.
use sqlx::PgPool;
use tor_books::repositories::{book_members::PgBookMembers, postgres::PgAuthoring};
use tor_books::services::authoring::AuthorError;
use tor_contracts::access::BookRole;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_book_lifecycle_membership_cas_and_deletion_gate() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    async fn user(pool: &PgPool, role: &str, marker: u128) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'S020 lifecycle','$argon2id$test') RETURNING id",
        )
        .bind(format!("s020-lifecycle-{role}-{marker}@example.test"))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    let owner = user(&pool, "owner", marker).await;
    let outsider = user(&pool, "outsider", marker).await;
    let translator = user(&pool, "translator", marker).await;
    let moderator = user(&pool, "moderator", marker).await;
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           visibility,access_level,project_status)
         VALUES($1,'ru',$2,$3,'draft','basic','planned') RETURNING id",
    )
    .bind(format!("s020:lifecycle:{marker}"))
    .bind(format!("Цикл книги {marker}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let unrelated: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id)
         VALUES($1,'ru',$2,$3) RETURNING id",
    )
    .bind(format!("s020:lifecycle:other:{marker}"))
    .bind(format!("Чужая книга {marker}"))
    .bind(outsider)
    .fetch_one(&pool)
    .await
    .unwrap();
    let members = PgBookMembers::new(pool.clone());
    members
        .add_collaborator(owner, book, translator, BookRole::Translator)
        .await
        .unwrap();
    members
        .add_collaborator(owner, book, moderator, BookRole::Moderator)
        .await
        .unwrap();
    let repo = PgAuthoring::new(pool.clone());
    let first = repo
        .manageable_lifecycle(owner, book)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.status, "in_progress");
    assert_eq!(first.revision, 1);
    assert!(repo
        .manageable_lifecycle(outsider, book)
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .manageable_lifecycle(translator, book)
        .await
        .unwrap()
        .is_some());
    assert!(repo
        .manageable_lifecycle(moderator, book)
        .await
        .unwrap()
        .is_some());
    assert!(repo
        .manageable_lifecycle(translator, unrelated)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.update_lifecycle(outsider, book, 1, "completed").await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.update_lifecycle(translator, unrelated, 1, "completed")
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.update_lifecycle(owner, book, 1, "pending_deletion")
            .await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        repo.update_lifecycle(owner, book, 1, "public").await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        repo.update_lifecycle(owner, book, 1, "completed").await,
        Ok(2)
    );
    assert_eq!(
        repo.update_lifecycle(translator, book, 1, "on_hiatus")
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        repo.update_lifecycle(translator, book, 2, "on_hiatus")
            .await,
        Ok(3)
    );
    assert_eq!(
        repo.update_lifecycle(moderator, book, 3, "abandoned").await,
        Ok(4)
    );
    let (status, revision, project, visibility, access): (String, i64, String, String, String) =
        sqlx::query_as(
            "SELECT lifecycle_status,lifecycle_revision,project_status,visibility,access_level
             FROM books WHERE id=$1",
        )
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "abandoned");
    assert_eq!(revision, 4);
    assert_eq!(project, "planned");
    assert_eq!(visibility, "draft");
    assert_eq!(access, "basic");

    members
        .revoke_collaborator(owner, book, moderator)
        .await
        .unwrap();
    assert!(repo
        .manageable_lifecycle(moderator, book)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.update_lifecycle(moderator, book, 4, "completed").await,
        Err(AuthorError::Missing)
    );

    // A pending deletion must not be undone by the ordinary Book settings page.
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.update_lifecycle(owner, book, 4, "in_progress").await,
        Err(AuthorError::Forbidden)
    );
    assert_eq!(
        repo.manageable_lifecycle(owner, book)
            .await
            .unwrap()
            .unwrap()
            .status,
        "pending_deletion"
    );

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .manageable_lifecycle(owner, book)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.update_lifecycle(translator, book, 4, "completed")
            .await,
        Err(AuthorError::Missing)
    );
    sqlx::query("UPDATE books SET admin_hold=FALSE, is_deleted=TRUE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .manageable_lifecycle(translator, book)
        .await
        .unwrap()
        .is_none());
    pool.close().await;
}
