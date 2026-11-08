//! E-07 / S009 disposable PG acceptance. No production DB access.
#![forbid(unsafe_code)]
use sqlx::{PgPool, Row};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_social::repositories::book_collections::{CollectionError, PgBookCollections};

async fn fixture() -> (PgPool, i64, i64, i64, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    for i in 0..2 {
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,$2,'$argon2id$fixture') RETURNING id",
        )
        .bind(format!("e07-{stamp}-{i}@example.invalid"))
        .bind(format!("E07User{stamp}{i}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        users.push(user);
    }
    let mut books = Vec::new();
    for (i, visibility, level) in [
        (0, "public", "basic"),
        (1, "draft", "basic"),
        (2, "public", "adult"),
    ] {
        let book:i64=sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
             VALUES($1,'ru',$2,$3,$4,$5) RETURNING id"
        ).bind(format!("ci:e07:{stamp}:{i}"))
            .bind(format!("E07 Book {i}"))
            .bind(visibility).bind(level).bind(users[0])
            .fetch_one(&pool).await.unwrap();
        books.push(book);
    }
    (pool, users[0], users[1], books[0], books[1], books[2])
}
fn key(n: u64) -> String {
    format!("{n:043}")
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn e_07_private_collection_owner_cas_and_public_projection() {
    let (pool, owner, other, public_book, draft_book, adult_book) = fixture().await;
    let collections = PgBookCollections::new(pool.clone());
    let idempotency = key(public_book as u64);
    let private = collections
        .create(owner, "My shelf", "private", &idempotency)
        .await
        .unwrap();
    assert_eq!(private.revision, 1);
    assert_eq!(
        collections
            .create(owner, "My shelf", "private", &idempotency)
            .await
            .unwrap()
            .id,
        private.id
    );
    assert_eq!(
        collections
            .create(owner, "Different", "private", &idempotency)
            .await,
        Err(CollectionError::Conflict)
    );
    assert_eq!(
        collections.detail(None, private.id).await.err(),
        Some(CollectionError::NotFound)
    );
    assert_eq!(
        collections.detail(Some(other), private.id).await.err(),
        Some(CollectionError::NotFound)
    );
    assert_eq!(
        collections
            .list(Some(owner))
            .await
            .unwrap()
            .iter()
            .filter(|c| c.id == private.id)
            .count(),
        1
    );
    assert!(collections
        .list(Some(other))
        .await
        .unwrap()
        .iter()
        .all(|c| c.id != private.id));
    assert_eq!(
        collections
            .update_metadata_if_current(other, private.id, 1, "Hi", "public")
            .await,
        Err(CollectionError::NotFound)
    );
    assert_eq!(
        collections.add_book(owner, private.id, 1, draft_book).await,
        Err(CollectionError::NotFound)
    );
    assert_eq!(
        collections.add_book(owner, private.id, 1, adult_book).await,
        Err(CollectionError::NotFound)
    );
    let choices = collections
        .find_addable_books(owner, private.id, "E07 Book 0")
        .await
        .unwrap();
    assert!(choices.iter().any(|b| b.id == public_book));
    assert!(collections
        .find_addable_books(other, private.id, "E07 Book 0")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        collections.find_addable_books(owner, private.id, "X").await,
        Err(CollectionError::Invalid)
    );
    let revision = collections
        .add_book(owner, private.id, 1, public_book)
        .await
        .unwrap();
    assert_eq!(revision, 2);
    assert!(!collections
        .find_addable_books(owner, private.id, "E07 Book 0")
        .await
        .unwrap()
        .iter()
        .any(|b| b.id == public_book));
    assert_eq!(
        collections
            .add_book(owner, private.id, 1, public_book)
            .await,
        Err(CollectionError::Conflict)
    );
    let detail = collections.detail(Some(owner), private.id).await.unwrap();
    assert!(detail.viewer_is_owner);
    assert_eq!(detail.books.len(), 1);
    assert_eq!(detail.books[0].id, public_book);
    assert_eq!(
        collections
            .update_metadata_if_current(owner, private.id, 1, "Old", "public")
            .await,
        Err(CollectionError::Conflict)
    );
    assert_eq!(
        collections
            .update_metadata_if_current(owner, private.id, 2, "Public shelf", "public")
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        collections
            .detail(None, private.id)
            .await
            .unwrap()
            .books
            .len(),
        1
    );
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(public_book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(collections
        .detail(None, private.id)
        .await
        .unwrap()
        .books
        .is_empty());
    assert_eq!(
        collections
            .delete_owned_collection(other, private.id, 3)
            .await,
        Err(CollectionError::NotFound)
    );
    collections
        .delete_owned_collection(owner, private.id, 3)
        .await
        .unwrap();
    assert_eq!(
        collections.detail(Some(owner), private.id).await.err(),
        Some(CollectionError::NotFound)
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn e_07_racing_owner_mutations_have_one_winner_and_no_entitlements() {
    let (pool, owner, _other, public_book, _draft, _adult) = fixture().await;
    let service = PgBookCollections::new(pool.clone());
    let key = key(owner as u64);
    let col = service
        .create(owner, "Racing", "public", &key)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        service.add_book(owner, col.id, 1, public_book),
        service.add_book(owner, col.id, 1, public_book)
    );
    assert_eq!([a, b].iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        [a, b]
            .iter()
            .filter(|r| **r == Err(CollectionError::Conflict))
            .count(),
        1
    );
    assert_eq!(service.detail(None, col.id).await.unwrap().books.len(), 1);
    let row = sqlx::query("SELECT revision FROM social_book_collections WHERE id=$1")
        .bind(col.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.try_get::<i64, _>("revision").unwrap(), 2);
    // This social write must not mint direct or Layer reading rights.
    let has_purchase: i64 =
        sqlx::query_scalar("SELECT count(*) FROM library_purchased_chapters WHERE user_id=$1")
            .bind(owner)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(has_purchase, 0);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn e_07_rejects_invalid_or_missing_ownership_without_writes() {
    let (pool, owner, other, book, _, _) = fixture().await;
    let service = PgBookCollections::new(pool);
    assert_eq!(
        service.create(owner, "  ", "private", &key(7)).await,
        Err(CollectionError::Invalid)
    );
    assert_eq!(
        service.create(owner, "Good", "hidden", &key(8)).await,
        Err(CollectionError::Invalid)
    );
    assert_eq!(
        service.create(owner, "Good", "private", "bad").await,
        Err(CollectionError::Invalid)
    );
    let col = service
        .create(owner, "Good", "private", &key(9))
        .await
        .unwrap();
    assert_eq!(
        service.add_book(other, col.id, 1, book).await,
        Err(CollectionError::NotFound)
    );
    assert_eq!(
        service
            .update_metadata_if_current(owner, col.id, 0, "X", "public")
            .await,
        Err(CollectionError::Invalid)
    );
    assert_eq!(
        service.remove_book_if_current(other, col.id, 1, book).await,
        Err(CollectionError::NotFound)
    );
    assert_eq!(
        service.detail(None, col.id).await.err(),
        Some(CollectionError::NotFound)
    );
}
