//! C-08/BR-060: saving a Book pins its released free chapter editions.
//! Disposable PG only; NOT evidence of completed ordinary deletion or reader UI.
use sqlx::{PgPool, Row};
use tor_reading::{repositories::PgLibrary, services::progress::ReadingPlace};

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
    let mut token_a = [0_u8; 32];
    token_a[..16].copy_from_slice(&nonce.to_le_bytes());
    token_a[16..].copy_from_slice(&nonce.to_le_bytes());
    let mut token_b = token_a;
    token_b[31] ^= 1;
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,now()+interval '1 hour'),($3,$4,now()+interval '1 hour')",
    )
    .bind(user_a)
    .bind(token_a.as_slice())
    .bind(user_b)
    .bind(token_b.as_slice())
    .execute(&pool)
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

    // Import chapter 2 first: visible TOC follows chapter numbering, not IDs.
    let (free2, v2) = chapter(&pool, book, 2, "public", "free", true).await;
    let (free1, v1) = chapter(&pool, book, 1, "public", "free", true).await;
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
    assert_eq!(pins(&pool, user_a).await, vec![(free2, v2), (free1, v1)]);
    let saved_first = library
        .pinned_free_chapter(user_a, free1, &token_a)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved_first.version_id, v1);
    assert_eq!(saved_first.body_markdown, "Original chapter 1");
    assert!(saved_first.saved_place.is_none());
    // A cursor only restores its own immutable edition, never creates access.
    sqlx::query(
        "INSERT INTO reader_book_positions
         (user_id,book_id,chapter_id,chapter_no,chapter_revision,
          version_id,block_index,char_offset)
         VALUES($1,$2,$3,1,1,$4,1,8)",
    )
    .bind(user_a)
    .bind(book)
    .bind(free1)
    .bind(v1)
    .execute(&pool)
    .await
    .unwrap();
    let anchored = library
        .pinned_free_chapter(user_a, free1, &token_a)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        anchored.saved_place,
        Some(ReadingPlace {
            block_index: 1,
            char_offset: 8,
        })
    );
    assert!(library
        .pinned_free_chapter(user_b, free1, &token_b)
        .await
        .unwrap()
        .is_none());
    assert!(library
        .pinned_free_chapter(user_a, paid, &token_a)
        .await
        .unwrap()
        .is_none());
    let one = library
        .pinned_free_chapters(user_a, book, 1, None, &token_a)
        .await
        .unwrap();
    assert_eq!(one.items.len(), 1);
    assert_eq!(one.items[0].chapter_no, 1);
    assert_eq!(one.next_cursor, Some(1));
    let next = library
        .pinned_free_chapters(user_a, book, 1, one.next_cursor, &token_a)
        .await
        .unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].chapter_no, 2);
    assert!(next.next_cursor.is_none());
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
    assert_eq!(pins(&pool, user_a).await, vec![(free2, v2), (free1, v1)]);
    let historical = library
        .pinned_free_chapter(user_a, free1, &token_a)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(historical.version_id, v1);
    assert_eq!(historical.body_markdown, "Original chapter 1");

    // A newly available free chapter is pinned on a deliberate replay only.
    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
        .bind(draft)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(pins(&pool, user_a).await.len(), 2);
    library.add_book(user_a, book).await.unwrap();
    assert_eq!(pins(&pool, user_a).await.len(), 3);
    assert!(!pins(&pool, user_a)
        .await
        .iter()
        .any(|(id, _)| { [paid, future, held, deleted, unreleased].contains(id) }));

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
    assert_eq!(pins(&pool, user_a).await[1], (free1, v1));
    let still_readable = library
        .pinned_free_chapter(user_a, free1, &token_a)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still_readable.version_id, v1);
    // Completed ordinary removal preserves the reader's exact saved-free V1.
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         ordinary_deleted_at=now(), visibility='draft' WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let tombstone = library.list_books(user_a, 10, None).await.unwrap();
    assert_eq!(tombstone.items.len(), 1);
    assert!(!tombstone.items[0].pending_deletion);
    assert!(tombstone.items[0].ordinary_deleted);
    assert!(library.add_book(user_a, book).await.is_err());
    let pinned = library
        .pinned_free_chapter(user_a, free1, &token_a)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pinned.version_id, v1);
    assert_eq!(pinned.body_markdown, "Original chapter 1");
    assert_eq!(
        pinned.saved_place,
        Some(ReadingPlace {
            block_index: 1,
            char_offset: 8,
        })
    );
    // Repointed public V2 progress is never mapped onto older saved V1.
    sqlx::query("UPDATE reader_book_positions SET version_id=$2 WHERE user_id=$1 AND book_id=$3")
        .bind(user_a)
        .bind(new_version)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(library
        .pinned_free_chapter(user_a, free1, &token_a)
        .await
        .unwrap()
        .unwrap()
        .saved_place
        .is_none());
    assert_eq!(
        library
            .pinned_free_chapters(user_a, book, 50, None, &token_a)
            .await
            .unwrap()
            .items
            .len(),
        3
    );
    assert!(library
        .pinned_free_chapter(user_b, free1, &token_b)
        .await
        .unwrap()
        .is_some());

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(library
        .pinned_free_chapter(user_a, free1, &token_a)
        .await
        .unwrap()
        .is_none());
    assert!(library
        .pinned_free_chapters(user_a, book, 50, None, &token_a)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query("UPDATE books SET admin_hold=FALSE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(library.add_book(user_a, book).await.is_err());
    // A revoked session cannot read an existing pinned V1 or its private TOC,
    // even though the account, Book and immutable pin remain active.
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE user_id=$1 AND token_hash=$2")
        .bind(user_a)
        .bind(token_a.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert!(library
        .pinned_free_chapter(user_a, free1, &token_a)
        .await
        .unwrap()
        .is_none());
    assert!(library
        .pinned_free_chapters(user_a, book, 50, None, &token_a)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(pins(&pool, user_a).await[1], (free1, v1));
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


#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn br060_mixed_archive_only_lists_owned_versions_after_ordinary_deletion() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Mixed archive','$argon2id$test') RETURNING id",
    )
    .bind(format!("br060-mixed-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Outsider','$argon2id$test') RETURNING id",
    )
    .bind(format!("br060-mixed-outsider-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut digest = [0_u8; 32];
    digest[..16].copy_from_slice(&nonce.to_le_bytes());
    digest[16..].copy_from_slice(&nonce.to_le_bytes());
    let mut other_digest = digest;
    other_digest[31] ^= 0x5a;
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,now()+interval '1 hour'),($3,$4,now()+interval '1 hour')",
    )
    .bind(actor)
    .bind(digest.as_slice())
    .bind(outsider)
    .bind(other_digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','Mixed archive','public','basic') RETURNING id",
    )
    .bind(format!("ci:br060:mixed:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let (free_id, free_v1) = chapter(&pool, book, 1, "public", "free", true).await;
    let (paid_id, paid_v1) = chapter(&pool, book, 2, "public", "paid", true).await;
    let (not_owned_id, _) = chapter(&pool, book, 3, "public", "paid", true).await;
    let (another_free_id, _) = chapter(&pool, book, 4, "public", "free", true).await;

    let library = PgLibrary::new(pool.clone());
    library.add_book(actor, book).await.unwrap();
    let paid_purchase: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
         (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,$4,12900,'RUB') RETURNING id",
    )
    .bind(actor)
    .bind(paid_id)
    .bind(paid_v1)
    .bind(format!("br060-mixed-purchase-{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let updated_free: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Not pinned new free V2') RETURNING id",
    )
    .bind(free_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(free_id)
        .bind(updated_free)
        .execute(&pool)
        .await
        .unwrap();
    // Remove the extra free pin to verify it does not enter via the public TOC.
    sqlx::query("DELETE FROM reader_saved_free_chapters WHERE user_id=$1 AND chapter_id=$2")
        .bind(actor)
        .bind(another_free_id)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         ordinary_deleted_at=now(),visibility='draft' WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let first = library
        .saved_archive_chapters(actor, book, 1, None, &digest)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].chapter_id, free_id);
    assert_eq!(first.items[0].saved_free_version_no, Some(1));
    assert!(first.items[0].purchased.is_none());
    assert_eq!(first.next_cursor, Some(1));
    let second = library
        .saved_archive_chapters(actor, book, 1, first.next_cursor, &digest)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].chapter_id, paid_id);
    assert!(second.items[0].saved_free_version_no.is_none());
    assert_eq!(second.items[0].purchased.as_ref().unwrap().purchase_id, paid_purchase);
    assert_eq!(second.next_cursor, None);
    let all = library
        .saved_archive_chapters(actor, book, 50, None, &digest)
        .await
        .unwrap();
    assert_eq!(all.items.len(), 2);
    assert!(all.items.iter().all(|c| ![not_owned_id, another_free_id].contains(&c.chapter_id)));
    assert!(library
        .saved_archive_chapters(outsider, book, 50, None, &other_digest)
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(library
        .saved_archive_chapters(actor, book, 50, None, &other_digest)
        .await
        .unwrap()
        .items
        .is_empty());
    let pinned = library.pinned_free_chapter(actor, free_id, &digest).await.unwrap().unwrap();
    assert_eq!(pinned.version_id, free_v1);

    // A refund suppresses paid navigation, but never erases the free pin.
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,$2,'refund')",
    )
    .bind(paid_purchase)
    .bind(format!("br060-refund-{nonce}"))
    .execute(&pool)
    .await
    .unwrap();
    let refunded = library
        .saved_archive_chapters(actor, book, 50, None, &digest)
        .await
        .unwrap();
    assert_eq!(refunded.items.len(), 1);
    assert_eq!(refunded.items[0].chapter_id, free_id);
    pool.close().await;
}
