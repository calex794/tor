//! AUTH-READ-02c1: real PG bounded discovery and terminal outcomes.
use super::{fixture, guest_prefix, update};
use sha2::{Digest, Sha256};
use tor_books::{
    repositories::{
        guest_prefix::{
            GuestPrefixPreparation, GuestPrefixTarget, PgGuestPrefix, MAX_GUEST_PREFIX_SWEEP_PAGE,
        },
        postgres::PgAuthoring,
    },
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest, SavedChapter},
};
use tor_contracts::guest_prefix::GUEST_PREFIX_ALGORITHM;

async fn add(
    author: &PgAuthoring,
    owner: i64,
    book: i64,
    number: i32,
    key: char,
    body: &str,
) -> SavedChapter {
    let key = key.to_string().repeat(43);
    let input = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number,
        expected_revision: 0,
        title: "Guest prefix sweep",
        body,
        access: "free",
        price_amount: 0,
        command_key: &key,
    })
    .unwrap();
    author
        .create_published_free_chapter(owner, &input)
        .await
        .unwrap()
}

fn target(book: i64, saved: &SavedChapter, revision: i64) -> GuestPrefixTarget {
    GuestPrefixTarget {
        book_id: book,
        chapter_id: saved.chapter_id,
        version_id: saved.version_id,
        policy_revision: revision,
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL18 /tor_ci"]
async fn auth_read_sweep_advances_over_skips_and_terminal_results() {
    let pool = guest_prefix::pool().await;
    let (author, owner, _, book, original) = fixture(&pool, "guest-prefix-sweep").await;
    let repo = PgGuestPrefix::new(pool.clone());
    assert_eq!(
        repo.missing_page(book, 1, 0, 2).await,
        Err(AuthorError::Conflict)
    );
    guest_prefix::prepare_book(&pool, book).await;
    let held = add(&author, owner, book, 8, 'b', "Held chapter body").await;
    let unsupported_body = "Start ![image](https://storage.example/private-key) end";
    let unsupported = add(&author, owner, book, 9, 'c', unsupported_body).await;
    let delayed = add(&author, owner, book, 11, 'd', "Delayed chapter body").await;
    let later = add(&author, owner, book, 14, 'e', "Later chapter body").await;
    sqlx::query("UPDATE chapters SET admin_hold=TRUE WHERE id=$1")
        .bind(held.chapter_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE chapters SET reader_released_at=clock_timestamp()+interval '1 day' WHERE id=$1",
    )
    .bind(delayed.chapter_id)
    .execute(&pool)
    .await
    .unwrap();

    let page = repo.missing_page(book, 1, 0, 2).await.unwrap();
    assert_eq!(page.targets, vec![target(book, &original, 1)]);
    assert_eq!(page.next_after_chapter_no, Some(8));
    assert_eq!(
        repo.prepare_current(page.targets[0], "Оригинальный текст V1 😀")
            .await,
        Ok(GuestPrefixPreparation::Stored)
    );
    let page = repo.missing_page(book, 1, 0, 2).await.unwrap();
    assert!(page.targets.is_empty());
    assert_eq!(page.next_after_chapter_no, Some(8));

    let page = repo.missing_page(book, 1, 8, 2).await.unwrap();
    assert_eq!(page.targets, vec![target(book, &unsupported, 1)]);
    assert_eq!(page.next_after_chapter_no, Some(11));
    for _ in 0..2 {
        assert_eq!(
            repo.prepare_current(page.targets[0], unsupported_body).await,
            Ok(GuestPrefixPreparation::NoExcerpt)
        );
    }
    let stored: (String, i32, String, Vec<u8>) = sqlx::query_as(
        "SELECT projection_status,visible_scalar_count,prefix_text,source_sha256
         FROM chapter_version_guest_prefixes
         WHERE chapter_id=$1 AND version_id=$2 AND algorithm_version=$3",
    )
    .bind(unsupported.chapter_id)
    .bind(unsupported.version_id)
    .bind(GUEST_PREFIX_ALGORITHM)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored.0, "unsupported");
    assert_eq!(stored.1, 0);
    assert!(stored.2.is_empty());
    let expected_hash: [u8; 32] = Sha256::digest(unsupported_body.as_bytes()).into();
    assert_eq!(stored.3.as_slice(), expected_hash.as_slice());
    let page = repo.missing_page(book, 1, 8, 2).await.unwrap();
    assert!(page.targets.is_empty());
    assert_eq!(page.next_after_chapter_no, Some(11));

    let page = repo.missing_page(book, 1, 11, 2).await.unwrap();
    assert_eq!(page.targets, vec![target(book, &later, 1)]);
    assert_eq!(page.next_after_chapter_no, None);
    assert_eq!(
        repo.missing_page(book, 1, 0, 0).await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        repo.missing_page(book, 1, 0, MAX_GUEST_PREFIX_SWEEP_PAGE + 1)
            .await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        repo.missing_page(book, 1, -1, 2).await,
        Err(AuthorError::Invalid)
    );

    // A new exact edition appears in a fresh sweep; historical ready
    // projections and terminal outcomes do not mask it.
    let edited = author
        .edit_published_free_chapter(
            owner,
            &update(
                book,
                original.chapter_id,
                original.revision,
                "Second edition",
                "New source for second edition",
                &"f".repeat(43),
            ),
        )
        .await
        .unwrap();
    assert_ne!(edited.version_id, original.version_id);
    let page = repo.missing_page(book, 1, 0, 2).await.unwrap();
    assert_eq!(page.targets, vec![target(book, &edited, 1)]);
    // Closing a new revision invalidates old cursors/targets. Canonical
    // chapter numbers, not row ranks, determine the new start boundary.
    sqlx::query(
        "UPDATE books SET guest_policy_revision=2,guest_open_through_chapter_no=9 WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.missing_page(book, 1, 0, 2).await,
        Err(AuthorError::Conflict)
    );
    let page = repo.missing_page(book, 2, 0, 2).await.unwrap();
    assert_eq!(page.targets, vec![target(book, &later, 2)]);
    assert_eq!(page.next_after_chapter_no, Some(14));
    let end = repo.missing_page(book, 2, 14, 2).await.unwrap();
    assert!(end.targets.is_empty());
    assert_eq!(end.next_after_chapter_no, None);

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.missing_page(book, 2, 0, 2).await,
        Err(AuthorError::Conflict)
    );
}
