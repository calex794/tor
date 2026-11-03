//! S020: active per-Book editor may update annotation, without owner privileges.
use sqlx::PgPool;
use tor_books::repositories::{book_members::PgBookMembers, postgres::PgAuthoring};
use tor_books::services::authoring::AuthorError;
use tor_contracts::access::BookRole;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_active_book_members_edit_description_and_revoked_members_cannot() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    for (suffix, name) in [
        ("owner", "Book owner"),
        ("translator", "Book translator"),
        ("moderator", "Book moderator"),
        ("outsider", "Site outsider"),
    ] {
        let actor: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,$2,'$argon2id$test') RETURNING id",
        )
        .bind(format!("s020-description-{suffix}-{marker}@example.test"))
        .bind(name)
        .fetch_one(&pool)
        .await
        .unwrap();
        users.push(actor);
    }
    let [owner, translator, moderator, outsider] =
        <[i64; 4]>::try_from(users).unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           description,visibility,access_level)
         VALUES($1,'ru',$2,$3,'Описание 0','draft','basic') RETURNING id",
    )
    .bind(format!("s020:description:{marker}"))
    .bind(format!("Описание книги {marker}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let other_book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id)
         VALUES($1,'ru',$2,$3) RETURNING id",
    )
    .bind(format!("s020:other-description:{marker}"))
    .bind(format!("Чужая книга {marker}"))
    .bind(outsider)
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
    let books = PgAuthoring::new(pool.clone());
    assert!(books
        .manageable_book_description(translator, book_id)
        .await
        .unwrap()
        .is_some());
    assert!(books
        .manageable_book_description(moderator, book_id)
        .await
        .unwrap()
        .is_some());
    assert!(books
        .manageable_book_description(outsider, book_id)
        .await
        .unwrap()
        .is_none());
    assert!(books
        .manageable_book_description(translator, other_book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        books
            .update_book_description(outsider, book_id, "Denied")
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        books
            .update_book_description(translator, other_book_id, "Denied")
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        books
            .update_book_description(translator, book_id, "Аннотация переводчика")
            .await,
        Ok(())
    );
    assert_eq!(
        books
            .update_book_description(moderator, book_id, "Аннотация модератора")
            .await,
        Ok(())
    );
    let (description, visibility, access): (String, String, String) = sqlx::query_as(
        "SELECT description,visibility,access_level FROM books WHERE id=$1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(description, "Аннотация модератора");
    assert_eq!(visibility, "draft");
    assert_eq!(access, "basic");

    members
        .revoke_collaborator(owner, book_id, moderator)
        .await
        .unwrap();
    assert!(books
        .manageable_book_description(moderator, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        books
            .update_book_description(moderator, book_id, "Denied after revoke")
            .await,
        Err(AuthorError::Missing)
    );
    assert!(books
        .manageable_book_description(translator, book_id)
        .await
        .unwrap()
        .is_some());

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(books
        .manageable_book_description(owner, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        books.update_book_description(translator, book_id, "Blocked").await,
        Err(AuthorError::Missing)
    );
    sqlx::query("UPDATE books SET admin_hold=FALSE,is_deleted=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(books
        .manageable_book_description(translator, book_id)
        .await
        .unwrap()
        .is_none());
    pool.close().await;
}
