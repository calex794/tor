//! C/H: 2050-chapter deleted-book archive and actor-scoped bidirectional keyset.
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_reading::{
    repositories::PgLibrary,
    services::library::{SavedArchiveTocCursor, SavedArchiveTocSide},
};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn saved_archive_toc_2050_chapters_uses_real_order_and_live_session() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let bookmarks = PgLibrary::new(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Archive cursor reader','$argon2id$ci') RETURNING id",
    )
    .bind(format!("toc-actor-{stamp}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Archive outsider','$argon2id$ci') RETURNING id",
    )
    .bind(format!("toc-outsider-{stamp}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut digest = [17u8; 32];
    digest[..16].copy_from_slice(&stamp.to_le_bytes());
    let mut other_digest = [23u8; 32];
    other_digest[..16].copy_from_slice(&stamp.to_le_bytes());
    for (id, token) in [(actor, &digest), (outsider, &other_digest)] {
        sqlx::query(
            "INSERT INTO sessions(token_hash,user_id,expires_at)
             VALUES($1,$2,now()+INTERVAL '2 hours')",
        )
        .bind(token.as_slice())
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    }

    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
         VALUES($1,'ru','2050 archived chapters','public','basic',$2) RETURNING id",
    ).bind(format!("ci:archive:keyset:{stamp}")).bind(actor)
     .fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
         SELECT $1,n,'Chapter '||n,'public','free'
         FROM generate_series(1,2050) AS n",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         SELECT id,1,'PRIVATE_PINNED_BODY_NOT_TOC'
         FROM chapters WHERE book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id)
         SELECT c.id,v.id FROM chapters c
         JOIN chapter_versions v ON v.chapter_id=c.id
         WHERE c.book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    bookmarks.add_book(actor, book).await.unwrap();
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         ordinary_deleted_at=now(),visibility='draft' WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    let chapter_id: i64 =
        sqlx::query_scalar("SELECT id FROM chapters WHERE book_id=$1 AND chapter_no=1805")
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
    let anchor = SavedArchiveTocCursor {
        chapter_no: 1805,
        chapter_id,
    };
    let initial_before = bookmarks
        .saved_archive_toc_window(
            actor,
            book,
            25,
            anchor,
            SavedArchiveTocSide::Before,
            &digest,
        )
        .await
        .unwrap();
    assert_eq!(initial_before.items.len(), 25);
    assert_eq!(initial_before.items.first().unwrap().chapter_no, 1780);
    assert_eq!(initial_before.items.last().unwrap().chapter_no, 1804);
    assert!(initial_before.next_cursor.is_some());
    let initial_after = bookmarks
        .saved_archive_toc_window(
            actor,
            book,
            25,
            anchor,
            SavedArchiveTocSide::AtOrAfter,
            &digest,
        )
        .await
        .unwrap();
    assert_eq!(initial_after.items.len(), 25);
    assert_eq!(initial_after.items.first().unwrap().chapter_no, 1805);
    assert_eq!(initial_after.items.last().unwrap().chapter_no, 1829);
    assert!(initial_after.next_cursor.is_some());

    let mut seen = HashSet::new();
    let mut cursor = anchor;
    let mut reached_start = false;
    for _ in 0..45 {
        let page = bookmarks
            .saved_archive_toc_window(
                actor,
                book,
                60,
                cursor,
                SavedArchiveTocSide::Before,
                &digest,
            )
            .await
            .unwrap();
        assert!(page.items.len() <= 60);
        assert!(page
            .items
            .windows(2)
            .all(|w| (w[0].chapter_no, w[0].chapter_id) < (w[1].chapter_no, w[1].chapter_id)));
        for item in &page.items {
            assert!(seen.insert(item.chapter_no), "duplicate before page");
            assert_eq!(item.saved_free_version_no, Some(1));
            assert!(item.purchased.is_none());
            assert!(!item.chapter_title.contains("PRIVATE_PINNED_BODY_NOT_TOC"));
        }
        if let Some(next) = page.next_cursor {
            cursor = SavedArchiveTocCursor::parse(&next).unwrap();
        } else {
            reached_start = true;
            break;
        }
    }
    assert!(reached_start);
    assert!(seen.contains(&1));
    cursor = anchor;
    let mut inclusive = true;
    let mut reached_end = false;
    for _ in 0..45 {
        let page = bookmarks
            .saved_archive_toc_window(
                actor,
                book,
                60,
                cursor,
                if inclusive {
                    SavedArchiveTocSide::AtOrAfter
                } else {
                    SavedArchiveTocSide::After
                },
                &digest,
            )
            .await
            .unwrap();
        inclusive = false;
        assert!(page.items.len() <= 60);
        assert!(page
            .items
            .windows(2)
            .all(|w| (w[0].chapter_no, w[0].chapter_id) < (w[1].chapter_no, w[1].chapter_id)));
        for item in &page.items {
            assert!(seen.insert(item.chapter_no), "duplicate after page");
            assert_eq!(item.saved_free_version_no, Some(1));
        }
        if let Some(next) = page.next_cursor {
            cursor = SavedArchiveTocCursor::parse(&next).unwrap();
        } else {
            reached_end = true;
            break;
        }
    }
    assert!(reached_end);
    assert_eq!(seen.len(), 2050);
    assert_eq!(seen.iter().copied().min(), Some(1));
    assert_eq!(seen.iter().copied().max(), Some(2050));

    // Someone who did not save the chapters cannot enumerate an owned archive.
    let outsider_page = bookmarks
        .saved_archive_toc_window(
            outsider,
            book,
            60,
            anchor,
            SavedArchiveTocSide::Before,
            &other_digest,
        )
        .await
        .unwrap();
    assert!(outsider_page.items.is_empty());
    // Changing age level, a moderation hold, or revoking the session must
    // instantly close both directions even with a previously issued cursor.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(bookmarks
        .saved_archive_toc_window(
            actor,
            book,
            60,
            anchor,
            SavedArchiveTocSide::Before,
            &digest
        )
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query("UPDATE books SET access_level='basic',admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(bookmarks
        .saved_archive_toc_window(actor, book, 60, anchor, SavedArchiveTocSide::After, &digest)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(bookmarks
        .saved_archive_toc_window(actor, book, 60, anchor, SavedArchiveTocSide::After, &digest)
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(bookmarks
        .saved_archive_toc_window(actor, book, 0, anchor, SavedArchiveTocSide::After, &digest)
        .await
        .is_err());
    assert!(bookmarks
        .saved_archive_toc_window(actor, book, 61, anchor, SavedArchiveTocSide::After, &digest)
        .await
        .is_err());
    pool.close().await;
}
