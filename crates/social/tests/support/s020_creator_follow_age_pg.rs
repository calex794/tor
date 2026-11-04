//! S020/E-05: creator follows do not bypass live Book age rights.
//! Run only against the disposable tor_ci PostgreSQL database.
use sqlx::PgPool;
use tor_social::services::follow::{CreatorFollowError, PgCreatorFollows};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn e_05_s020_creator_source_book_age_revocation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let author: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'E05 author','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("e05-author-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'E05 reader','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("e05-reader-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','E05 age protected source','public',$2,'basic') RETURNING id",
    )
    .bind(format!("ci:e05:s020:{nonce}"))
    .bind(author)
    .fetch_one(&pool)
    .await
    .unwrap();

    let follows = PgCreatorFollows::new(pool.clone());
    assert_eq!(follows.follow_from_book(reader, book).await, Ok(author));
    let original = follows.list_active(reader, None).await.unwrap();
    assert_eq!(original.len(), 1);
    assert_eq!(
        original[0]
            .source_book
            .as_ref()
            .map(|(_, title)| title.as_str()),
        Some("E05 age protected source"),
    );
    let period = original[0].period_id;

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound),
    );
    let denied = follows.list_active(reader, None).await.unwrap();
    assert_eq!(denied.len(), 1, "existing follow must be retained");
    assert_eq!(denied[0].period_id, period);
    assert_eq!(denied[0].source_book, None, "revocation hides title");

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(follows.follow_from_book(reader, book).await, Ok(author));
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_some());

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound),
    );
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_none());
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)
         ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE",
    )
    .bind(reader)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(follows.follow_from_book(reader, book).await, Ok(author));
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_some());

    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(reader)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound),
    );
    assert!(follows.list_active(reader, None).await.unwrap()[0]
        .source_book
        .is_none());
    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        follows.follow_from_book(reader, book).await,
        Err(CreatorFollowError::NotFound),
    );
    assert_eq!(
        follows.list_active(reader, None).await.unwrap()[0].source_book,
        None,
        "legacy NULL cannot become basic",
    );
    assert_eq!(follows.unfollow(reader, author).await, Ok(()));
    assert!(follows.list_active(reader, None).await.unwrap().is_empty());
    // No permanent migration or explicit deletion of immutable financial history.
}
