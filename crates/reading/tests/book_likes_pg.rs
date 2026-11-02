//! B: active book likes are separate from ratings, thanks and purchases.
#![forbid(unsafe_code)]

use sqlx::PgPool;
use tor_reading::repositories::{LikeMutation, PgBookLikes};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn book_like_duplicate_concurrent_unlike_visibility_and_erasure() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let first: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Book liker','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("book-liker-1-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let second: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Second book liker','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("book-liker-2-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','Проверка <like>','public') RETURNING id",
    )
    .bind(format!("ci:book-likes:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let likes = PgBookLikes::new(pool.clone());
    assert_eq!(likes.public_count(book).await.unwrap(), Some(0));
    assert_eq!(likes.public_count(0).await.unwrap(), None);
    assert_eq!(
        likes.set_like(first, 0, true).await.unwrap(),
        LikeMutation::NotVisible
    );

    // A concurrent double-click produces exactly one active row.
    let (a, b) = tokio::join!(
        likes.set_like(first, book, true),
        likes.set_like(first, book, true)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(
        (a == LikeMutation::Added && b == LikeMutation::AlreadyLiked)
            || (b == LikeMutation::Added && a == LikeMutation::AlreadyLiked)
    );
    assert_eq!(likes.public_count(book).await.unwrap(), Some(1));
    let first_liked = likes
        .visible_summary(first, book)
        .await
        .unwrap()
        .unwrap()
        .liked;
    assert!(first_liked);
    let second_liked = likes
        .visible_summary(second, book)
        .await
        .unwrap()
        .unwrap()
        .liked;
    assert!(!second_liked);

    assert_eq!(
        likes.set_like(second, book, true).await.unwrap(),
        LikeMutation::Added
    );
    assert_eq!(likes.public_count(book).await.unwrap(), Some(2));
    assert_eq!(
        likes.set_like(first, book, false).await.unwrap(),
        LikeMutation::Removed
    );
    assert_eq!(
        likes.set_like(first, book, false).await.unwrap(),
        LikeMutation::AlreadyRemoved
    );
    assert_eq!(likes.public_count(book).await.unwrap(), Some(1));
    assert_eq!(
        likes.set_like(first, book, true).await.unwrap(),
        LikeMutation::Added
    );

    // Moderation must hide both the public count and the personal action.
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(likes.public_count(book).await.unwrap(), None);
    assert!(likes.visible_summary(first, book).await.unwrap().is_none());
    assert_eq!(
        likes.set_like(first, book, false).await.unwrap(),
        LikeMutation::NotVisible
    );
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(likes.public_count(book).await.unwrap(), Some(2));

    // Account erasure removes only that account's row.
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(likes.public_count(book).await.unwrap(), Some(1));
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(likes.public_count(book).await.unwrap(), Some(0));
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(likes.public_count(book).await.unwrap(), None);
    pool.close().await;
}
