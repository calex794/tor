//! BR-060: a completed ordinary Book deletion preserves private V1 pins,
//! but a subsequent *legal* Book block or administrative hold must suspend
//! both archive reader routes without destroying their original entitlements.
use super::*;

fn retained_state_sql(book: i64, chapter: i64, purchase_id: i64, free_chapter: i64) -> String {
    format!(
        "SELECT concat_ws('|',
           COALESCE((SELECT active_version_id::text
                     FROM library_purchased_chapters WHERE purchase_id={purchase_id}),'NO_PAID'),
           COALESCE((SELECT version_id::text FROM reader_saved_free_chapters
                     WHERE chapter_id={free_chapter} AND user_id=(
                       SELECT user_id FROM chapter_purchase_facts WHERE id={purchase_id}
                     )),'NO_FREE'),
           COALESCE((SELECT version_id::text || ':' || char_offset::text || ':' || revision::text
                     FROM reader_book_positions WHERE book_id={book}
                       AND user_id=(SELECT user_id FROM chapter_purchase_facts
                                    WHERE id={purchase_id})),'NO_POSITION'),
           COALESCE((SELECT balance_minor::text FROM reader_wallet_accounts
                     WHERE user_id=(SELECT user_id FROM chapter_purchase_facts
                                    WHERE id={purchase_id})),'NO_WALLET'),
           (SELECT COUNT(*)::text FROM reader_wallet_entries WHERE purchase_id={purchase_id}),
           (SELECT COUNT(*)::text FROM chapter_purchase_revocations
            WHERE purchase_id={purchase_id}),
           (SELECT COUNT(*)::text FROM chapter_purchase_facts
            WHERE id={purchase_id} AND chapter_id={chapter})
         )"
    )
}

async fn assert_archive_denied(
    app: &Router,
    book: i64,
    chapter: i64,
    purchase_id: i64,
    free_chapter: i64,
    cookie: &str,
    paid_status: StatusCode,
) {
    let toc = private_get(app, &format!("/me/library/{book}/saved"), Some(cookie)).await;
    let body = html_body(toc).await;
    assert!(
        !body.contains(&format!("/me/library/saved/{free_chapter}"))
            && !body.contains(&format!("/read/paid/{chapter}?purchase_id={purchase_id}")),
        "legal/hard block may not expose archived pinned links"
    );

    let free = private_get(
        app,
        &format!("/me/library/saved/{free_chapter}"),
        Some(cookie),
    )
    .await;
    assert_eq!(free.status(), StatusCode::NOT_FOUND);
    assert!(
        !html_body(free).await.contains("Real public free content"),
        "blocked saved-free V1 must not be served"
    );

    let paid = paid_page(app, chapter, purchase_id, cookie).await;
    assert_eq!(paid.status(), paid_status);
    assert!(
        !html_body(paid).await.contains("data-paid-chapter-reader"),
        "blocked paid V1 must not produce a reader shell"
    );

    let purchases = private_get(app, "/me/purchases", Some(cookie)).await;
    assert_eq!(purchases.status(), StatusCode::OK);
    assert!(
        !html_body(purchases)
            .await
            .contains(&format!("/read/paid/{chapter}?purchase_id={purchase_id}")),
        "blocked purchase must not be advertised as currently readable"
    );
}

/// No extra signed grants: the existing paid-private fixture is deliberately
/// near its per-actor limiter. Both paid GET and saved-free GET exercise
/// the real Axum/PG entitlement gates; the suite's MinIO test follows below.
pub(super) async fn verify(
    app: &Router,
    url: &str,
    book: i64,
    chapter: i64,
    purchase_id: i64,
    free_chapter: i64,
    fresh_cookie: &str,
) {
    let snapshot_sql = retained_state_sql(book, chapter, purchase_id, free_chapter);
    let original = pg(url, &snapshot_sql);
    assert!(!original.contains("NO_PAID") && !original.contains("NO_FREE"));
    assert_eq!(
        pg(
            url,
            &format!("SELECT lifecycle_status FROM books WHERE id={book}")
        ),
        "ordinary_deleted"
    );

    pg(
        url,
        &format!("UPDATE books SET visibility_status='blocked' WHERE id={book}"),
    );
    assert_archive_denied(
        app,
        book,
        chapter,
        purchase_id,
        free_chapter,
        fresh_cookie,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(pg(url, &snapshot_sql), original, "legal hold is not deletion");
    pg(
        url,
        &format!("UPDATE books SET visibility_status='active' WHERE id={book}"),
    );

    pg(
        url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={book}"),
    );
    assert_archive_denied(
        app,
        book,
        chapter,
        purchase_id,
        free_chapter,
        fresh_cookie,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(pg(url, &snapshot_sql), original, "admin hold is not deletion");
    pg(
        url,
        &format!("UPDATE books SET admin_hold=FALSE WHERE id={book}"),
    );

    assert_eq!(pg(url, &snapshot_sql), original);
    assert_eq!(
        private_get(
            app,
            &format!("/me/library/saved/{free_chapter}"),
            Some(fresh_cookie),
        )
        .await
        .status(),
        StatusCode::OK,
        "lifting a hold restores the same pinned saved-free V1"
    );
    assert_eq!(
        paid_page(app, chapter, purchase_id, fresh_cookie)
            .await
            .status(),
        StatusCode::OK,
        "lifting a hold restores the original purchased V1"
    );
}
