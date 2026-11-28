//! Real PostgreSQL acceptance for the bounded catalog cursor.
use super::{
    guest_prefix_outbox::{fixture, publish},
    pg,
};
use tor_books::{
    repositories::{
        guest_prefix::{
            GuestPrefixSweepCursor::{Books, Chapters},
            PgGuestPrefix, MAX_GUEST_PREFIX_BOOK_PAGE,
        },
        postgres::PgAuthoring,
    },
    services::authoring::AuthorError,
};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn auth_read_sweep_step_rechecks_revision_and_advances_empty_book_page() {
    let (url, catalog, owner, book) = fixture().await;
    let pool = catalog.pool();
    let author = PgAuthoring::new(pool.clone());
    let prefix = PgGuestPrefix::new(pool.clone());
    let source = "Visible beginning. The private tail belongs only to the full chapter.";
    publish(&author, owner, book, 7, 'a', source).await;
    let start = Books {
        after_book_id: book - 1,
    };
    assert_eq!(
        prefix.sweep_step(Books { after_book_id: -1 }).await,
        Err(AuthorError::Invalid)
    );
    let discovered = prefix.sweep_step(start).await.unwrap();
    let stale = Chapters {
        book_id: book,
        policy_revision: 1,
        after_chapter_no: 0,
    };
    assert_eq!(discovered.next, Some(stale));
    assert_eq!(discovered.enqueued, 0);
    pg(
        &url,
        &format!("UPDATE books SET guest_policy_revision=2 WHERE id={book}"),
    );
    let skipped = prefix.sweep_step(stale).await.unwrap();
    assert!(skipped.stale_book);
    assert_eq!(skipped.enqueued, 0);
    let after_book = Books {
        after_book_id: book,
    };
    assert_eq!(skipped.next, Some(after_book));

    let current = prefix.sweep_step(start).await.unwrap().next.unwrap();
    assert_eq!(
        current,
        Chapters {
            book_id: book,
            policy_revision: 2,
            after_chapter_no: 0,
        }
    );
    let first = prefix.sweep_step(current).await.unwrap();
    assert_eq!(first.enqueued, 1);
    assert_eq!(first.next, Some(after_book));
    assert_eq!(prefix.sweep_step(current).await.unwrap().enqueued, 0);

    // A later publication is found when a new pass revisits the same book.
    publish(&author, owner, book, 8, 'b', source).await;
    assert_eq!(prefix.sweep_step(current).await.unwrap().enqueued, 1);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events
                 WHERE event_type='GuestPrefixPrepare' AND payload->>'book_id'='{book}'"
            ),
        ),
        "2"
    );
    // No consumer should inherit this test's unfinished events.
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET dead_at=clock_timestamp()
             WHERE event_type='GuestPrefixPrepare' AND payload->>'book_id'='{book}'"
        ),
    );

    // A full page of ordinary, unprepared books cannot cause a global
    // eligibility scan or an early end to the pass.
    let last: i64 = pg(
        &url,
        &format!(
            "WITH inserted AS (
                INSERT INTO books(source_identity,target_language,title,visibility,
                                  owner_user_id,access_level,guest_policy_revision)
                SELECT 'ci:auth-read-sweep:{book}:' || n::text,'ru','Unprepared book',
                       'public',{owner},'basic',0
                FROM generate_series(1,{MAX_GUEST_PREFIX_BOOK_PAGE}) AS n
                RETURNING id
             ) SELECT max(id) FROM inserted"
        ),
    )
    .parse()
    .unwrap();
    let empty = prefix.sweep_step(after_book).await.unwrap();
    assert_eq!(empty.enqueued, 0);
    assert!(!empty.stale_book);
    assert_eq!(
        empty.next,
        Some(Books {
            after_book_id: last,
        })
    );
}
