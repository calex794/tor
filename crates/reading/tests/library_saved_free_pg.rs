//! C-08/BR-060: saving a Book pins its released free chapter editions.
//! Disposable PG only; NOT evidence of completed ordinary deletion or reader UI.
use sqlx::{PgPool, Row};
use tor_reading::repositories::PgLibrary;

async fn chapter(
    pool: &PgPool,
    book_id: i64,
    number: i32,
    visibility: &str,
    access: &str,
    published: bool,
) -> (i64, i64) {
    let price: i64 = if access == "paid" { 12900 } else { 0 };
    let chapter_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,$3,$4,$5,$6) RETURNING id",
    )
    .bind(book_id)
    .bind(number)
    .bind(format!("Глава {number}"))
    .bind(visibility)
    .bind(access)
    .bind(price)
    .fetch_one(pool)
    .await
    .unwrap();
    let version_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    )
    .bind(chapter_id)
    .bind(format!("Original chapter {number}"))
    .fetch_one(pool)
    .await
    .unwrap();
    if published {
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(chapter_id)
            .bind(version_id)
            .execute(pool)
            .await
            .unwrap();
    }
    (chapter_id, version_id)
}

async fn pins(pool: &PgPool, user: i64) -> Vec<(i64, i64)> {
    let rows = sqlx::query(
        "SELECT chapter_id,version_id FROM reader_saved_free_chapters
         WHERE user_id=$1 ORDER BY chapter_id",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|r| (r.get("chapter_id"), r.get("version_id")))
        .collect()
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn c_08_add_book_pins_free_versions_and_preserves_v1_without_paid_grants() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci");
    assert!(url.ends_with("/tor_ci"), "never touch permanent data");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user_a: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'C08 A','$argon2id$test') RETURNING id",
    )
    .bind(format!("c08-saved-free-a-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let user_b: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'C08 B','$argon2id$test') RETURNING id",
    )
    .bind(format!("c08-saved-free-b-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','C08 saved free','public','basic') RETURNING id",
    )
    .bind(format!("ci:c08:saved-free:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let (free1, v1) = chapter(&pool, book, 1, "public", "free", true).await;
    let (free2, v2) = chapter(&pool, book, 2, "public", "free", true).await;
    let (paid, _) = chapter(&pool, book, 3, "public", "paid", true).await;
    let (draft, _) = chapter(&pool, book, 4, "draft", "free", true).await;
    let (future, _) = chapter(&pool, book, 5, "public", "free", true).await;
    let (held, _) = chapter(&pool, book, 6, "public", "free", true).await;
    let (deleted, _) = chapter(&pool, book, 7, "public", "free", true).await;
    let (unreleased, _) = chapter(&pool, book, 8, "public", "free", false).await;
    sqlx::query("UPDATE chapters SET reader_released_at=now()+interval '1 day' WHERE id=$1")
        .bind(future)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET admin_hold=TRUE WHERE id=$1")
        .bind(held)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET is_deleted=TRUE WHERE id=$1")
        .bind(deleted)
        .execute(&pool)
        .await
        .unwrap();
    let library = PgLibrary::new(pool.clone());
    library.add_book(user_a, book).await.unwrap();
    assert_eq!(pins(&pool, user_a).await, vec![(free1, v1), (free2, v2)]);
    let first = library.list_books(user_a, 10, None).await.unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].book_id, book);

    // Free V1 remains pinned after an author publishes V2; replay cannot
    // replace it with a new version or fabricate a paid entitlement.
    let new_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'NEW V2') RETURNING id",
    )
    .bind(free1)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(free1)
        .bind(new_version)
        .execute(&pool)
        .await
        .unwrap();
    library.add_book(user_a, book).await.unwrap();
    assert_eq!(pins(&pool, user_a).await, vec![(free1, v1), (free2, v2)]);

    // A newly available free chapter is pinned on a deliberate replay only.
    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
        .bind(draft)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(pins(&pool, user_a).await.len(), 2);
    library.add_book(user_a, book).await.unwrap();
    assert_eq!(pins(&pool, user_a).await.len(), 3);
    assert!(!pins(&pool, user_a).await.iter().any(|(id, _)| {
        [paid, future, held, deleted, unreleased].contains(id)
    }));

    library.add_book(user_b, book).await.unwrap();
    assert_eq!(pins(&pool, user_b).await.len(), 3);
    // An immutable chapter version must not be erased while any user has a pin.
    assert!(sqlx::query("DELETE FROM chapter_versions WHERE id=$1")
        .bind(v1)
        .execute(&pool)
        .await
        .is_err());

    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let saved = library.list_books(user_a, 10, None).await.unwrap();
    assert_eq!(saved.items.len(), 1);
    assert!(saved.items[0].pending_deletion);
    assert_eq!(pins(&pool, user_a).await[0], (free1, v1));
    assert!(library.add_book(user_a, book).await.is_err());
    library.remove_book(user_a, book).await.unwrap();
    assert!(pins(&pool, user_a).await.is_empty());
    assert_eq!(pins(&pool, user_b).await.len(), 3);
    library.remove_book(user_b, book).await.unwrap();
    assert!(pins(&pool, user_b).await.is_empty());
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id IN ($1,$2)")
        .bind(user_a)
        .bind(user_b)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
