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
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','Проверка <like>','public','basic') RETURNING id",
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

    // S020/P0 #486: neither a historical like nor a valid session is an
    // exemption from the Book's live 18+/21+ reader tier.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
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
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(likes.visible_summary(first, book).await.unwrap().is_some());
    assert!(likes.visible_summary(second, book).await.unwrap().is_none());

    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(likes.visible_summary(first, book).await.unwrap().is_none());
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(first)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(likes.visible_summary(first, book).await.unwrap().is_some());
    assert_eq!(
        likes.set_like(first, book, true).await.unwrap(),
        LikeMutation::AlreadyLiked
    );
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(likes.visible_summary(first, book).await.unwrap().is_none());
    assert_eq!(
        likes.set_like(first, book, false).await.unwrap(),
        LikeMutation::NotVisible
    );
    sqlx::query("UPDATE books SET access_level=NULL WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(likes.visible_summary(first, book).await.unwrap().is_none());

    // A site administrator has its own active role; book moderators do not.
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'moderator',TRUE)",
    )
    .bind(first)
    .execute(&pool)
    .await
    .unwrap();
    assert!(likes.visible_summary(first, book).await.unwrap().is_none());
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)",
    )
    .bind(first)
    .execute(&pool)
    .await
    .unwrap();
    assert!(likes.visible_summary(first, book).await.unwrap().is_some());
    sqlx::query("UPDATE account_roles SET is_active=FALSE WHERE user_id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert!(likes.visible_summary(first, book).await.unwrap().is_none());
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(likes.public_count(book).await.unwrap(), Some(2));

    // A book pending deletion is not publicly discoverable in NEwBOO.
    // Existing likes remain for recovery, but public reactions and mutations
    // must not disclose them. Direct Book reading is a separate entitlement.
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(likes.public_count(book).await.unwrap(), None);
    assert!(likes.visible_summary(first, book).await.unwrap().is_none());
    assert_eq!(likes.direct_view_count(book).await.unwrap(), Some(2));
    assert_eq!(
        likes
            .direct_view_summary(first, book)
            .await
            .unwrap()
            .map(|summary| summary.count),
        Some(2)
    );
    assert_eq!(
        likes.set_like(first, book, false).await.unwrap(),
        LikeMutation::NotVisible
    );
    let saved_likes: i64 = sqlx::query_scalar("SELECT count(*) FROM book_likes WHERE book_id=$1")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(saved_likes, 2);
    sqlx::query("UPDATE books SET lifecycle_status='in_progress' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(likes.public_count(book).await.unwrap(), Some(2));

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

    // The fixture's 21+ grant and roles use RESTRICT foreign keys;
    // clear only its authority rows before testing account erasure.
    sqlx::query("DELETE FROM user_level_grants WHERE user_id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM account_roles WHERE user_id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
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
