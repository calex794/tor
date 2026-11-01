//! C-08/S007: isolated PostgreSQL bookmarks, owner privacy and bounded keyset.
use std::collections::HashSet;
use tor_reading::{
    repositories::{PgLibrary, PgReader},
    services::{library::LibraryError, PublicReadingRepository},
};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL on disposable tor_ci database"]
async fn c_08_bookmarks_are_private_bounded_and_never_grant_paid_text() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    assert!(url.ends_with("/tor_ci"), "never touch production data");
    let pool = sqlx::PgPool::connect(&url).await.expect("tor_app");
    let bookmarks = PgLibrary::new(pool.clone());
    let reader = PgReader::new(pool.clone());
    sqlx::query(
        "INSERT INTO users (id,email_normalized,display_name,password_hash) \
         OVERRIDING SYSTEM VALUE VALUES \
         (92001,'c08-1@example.invalid','Reader A','$argon2id$test'), \
         (92002,'c08-2@example.invalid','Reader B','$argon2id$test')",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(
        bookmarks.add_book(92001, 2).await,
        Err(LibraryError::NotFound)
    );
    assert_eq!(
        bookmarks.add_book(92001, 3).await,
        Err(LibraryError::NotFound)
    );
    bookmarks.add_book(92001, 1).await.unwrap();
    bookmarks.add_book(92001, 1).await.unwrap();
    let own = bookmarks.list_books(92001, 30, None).await.unwrap();
    assert_eq!(own.items.len(), 1, "a replay must not duplicate a bookmark");
    assert_eq!(own.items[0].book_id, 1);
    assert!(bookmarks
        .list_books(92002, 30, None)
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(
        reader.public_chapter(103).await.unwrap().is_none(),
        "bookmark != paid right"
    );

    sqlx::query(
        "INSERT INTO books(id,source_identity,target_language,title,visibility) \
         SELECT n, 'ci:c08:' || n, 'ru', 'Длинный список ' || n, 'public' \
         FROM generate_series(92010, 92114) AS n",
    )
    .execute(&pool)
    .await
    .unwrap();
    // A large, real bookmark page must never scan another reader's row.
    sqlx::query(
        "INSERT INTO reader_library_items(user_id,book_id) \
         SELECT 92001, n FROM generate_series(92010, 92114) AS n",
    )
    .execute(&pool)
    .await
    .unwrap();
    bookmarks.add_book(92002, 92010).await.unwrap();
    let mut after = None;
    let mut seen = HashSet::new();
    let mut pages = 0;
    loop {
        let page = bookmarks.list_books(92001, 30, after).await.unwrap();
        pages += 1;
        assert!(page.items.len() <= 30);
        for item in &page.items {
            assert!(seen.insert(item.book_id), "duplicate item across pages");
        }
        after = page.next_cursor;
        if after.is_none() {
            break;
        }
        assert!(pages <= 5, "cursor did not terminate");
    }
    assert_eq!(pages, 4);
    assert_eq!(seen.len(), 106);
    assert_eq!(
        bookmarks.list_books(92001, 0, None).await.err(),
        Some(LibraryError::Invalid)
    );
    assert_eq!(
        bookmarks.list_books(92001, 101, None).await.err(),
        Some(LibraryError::Invalid)
    );

    // Removing A's copy must not remove B's independently saved copy.
    // Check while visible: a moderation hold would otherwise mask deletion.
    bookmarks.remove_book(92001, 92010).await.unwrap();
    let other = bookmarks.list_books(92002, 30, None).await.unwrap();
    assert_eq!(other.items.len(), 1);
    assert_eq!(other.items[0].book_id, 92010);
    bookmarks.add_book(92001, 92010).await.unwrap();

    // A book withdrawn from publication must disappear even if saved.
    sqlx::query("UPDATE books SET admin_hold = true WHERE id = 92010")
        .execute(&pool)
        .await
        .unwrap();
    let page = bookmarks.list_books(92001, 100, None).await.unwrap();
    assert!(!page.items.iter().any(|book| book.book_id == 92010));
    bookmarks.remove_book(92001, 92010).await.unwrap();
    bookmarks.remove_book(92001, 92010).await.unwrap();
    let other = bookmarks.list_books(92002, 30, None).await.unwrap();
    assert!(!other.items.iter().any(|book| book.book_id == 92010));
    bookmarks.remove_book(92001, 1).await.unwrap();
    assert!(!bookmarks
        .list_books(92001, 100, None)
        .await
        .unwrap()
        .items
        .iter()
        .any(|book| book.book_id == 1));

    sqlx::query("DELETE FROM users WHERE id IN (92001,92002)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id BETWEEN 92010 AND 92114")
        .execute(&pool)
        .await
        .unwrap();
}
