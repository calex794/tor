//! S020: owner edits all translated/original aliases without leaking drafts.
use sqlx::PgPool;
use tor_books::repositories::postgres::PgAuthoring;
use tor_books::services::authoring::AuthorError;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_owner_titles_update_claims_atomically_across_languages() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 Titles','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-title-owner-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 Other','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-title-other-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let title = format!("Первый заголовок {marker}");
    let renamed_title = format!("Обновлённый заголовок {marker}");
    let chinese = format!("修仙之路{marker}");
    let english = format!("The Immortal Path {marker}");
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           visibility,access_level)
         VALUES($1,'ru',$2,$3,'draft','basic') RETURNING id",
    )
    .bind(format!("s020:titles:{marker}"))
    .bind(&title)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgAuthoring::new(pool.clone());
    assert!(repo.owned_book_titles(other, book).await.unwrap().is_none());
    assert_eq!(
        repo.update_book_titles(other, book, 1, "No", "", ["", "", ""])
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.update_book_titles(owner, book, 1, &renamed_title, &chinese, [&english, "", ""])
            .await,
        Ok(2)
    );
    assert_eq!(
        repo.update_book_titles(owner, book, 1, "Stale", "", ["", "", ""])
            .await,
        Err(AuthorError::Conflict)
    );
    let claims: Vec<String> = sqlx::query_scalar(
        "SELECT normalized_title FROM book_registration_titles WHERE book_id=$1
         ORDER BY normalized_title",
    )
    .bind(book)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(claims.len(), 3);
    assert!(claims.contains(&chinese.to_lowercase()));
    assert!(claims.contains(&english.to_lowercase()));
    let (visibility, access, revision): (String, String, i64) = sqlx::query_as(
        "SELECT visibility, access_level, title_metadata_revision
                        FROM books WHERE id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(visibility, "draft");
    assert_eq!(access, "basic");
    assert_eq!(revision, 2);
    let conflict: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id)
         VALUES($1,'ru',$2,$3) RETURNING id",
    )
    .bind(format!("s020:other:{marker}"))
    .bind(format!("Another book {marker}"))
    .bind(other)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.update_book_titles(other, conflict, 1, "No", &chinese, ["", "", ""])
            .await,
        Err(AuthorError::Conflict)
    );
    // Old books with no claimed aliases also block a new collision.
    let old = format!("Исторический оригинал {marker}");
    sqlx::query("INSERT INTO books(source_identity,target_language,title) VALUES($1,'ru',$2)")
        .bind(format!("s020:old:{marker}"))
        .bind(&old)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.update_book_titles(owner, book, 2, "Different", &old, ["", "", ""])
            .await,
        Err(AuthorError::Conflict)
    );
    let title_after: String = sqlx::query_scalar("SELECT title FROM books WHERE id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title_after, renamed_title);
    let _ = sqlx::query("SELECT id FROM books WHERE id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
}
