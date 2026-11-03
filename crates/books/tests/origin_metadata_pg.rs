//! S020: origin metadata ownership, optimistic revision, visibility isolation.
use sqlx::PgPool;
use tor_books::repositories::postgres::PgAuthoring;
use tor_books::services::authoring::AuthorError;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_owner_origin_metadata_preserves_access_and_publication() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Origin owner','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-origin-owner-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Origin other','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-origin-other-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           visibility,access_level,original_language,release_year,catalog_section)
         VALUES($1,'ru',$2,$3,'draft','basic','zh',2022,'novels/chinese')
         RETURNING id",
    )
    .bind(format!("s020:origin:{marker}"))
    .bind(format!("Оригинал книги {marker}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let books = PgAuthoring::new(pool.clone());
    let initial = books
        .owned_origin_metadata(owner, book_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.original_language, "zh");
    assert_eq!(initial.release_year, Some(2022));
    assert_eq!(initial.revision, 1);
    assert!(books
        .owned_origin_metadata(outsider, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        books
            .update_origin_metadata(outsider, book_id, 1, "en", "2025", "novels/english")
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        books
            .update_origin_metadata(owner, book_id, 1, "EN", "2025", "novels/english")
            .await,
        Ok(2)
    );
    assert_eq!(
        books
            .update_origin_metadata(owner, book_id, 1, "zh", "2024", "novels/chinese")
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        books
            .update_origin_metadata(owner, book_id, 2, "zh", "invalid", "")
            .await,
        Err(AuthorError::Invalid)
    );
    let (language, year, section, revision, visibility, access): (
        String,
        Option<i32>,
        String,
        i64,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT original_language,release_year,catalog_section,
                origin_metadata_revision,visibility,access_level
         FROM books WHERE id=$1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(language, "en");
    assert_eq!(year, Some(2025));
    assert_eq!(section, "novels/english");
    assert_eq!(revision, 2);
    assert_eq!(visibility, "draft");
    assert_eq!(access, "basic");
    pool.close().await;
}
