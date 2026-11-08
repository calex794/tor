//! C-02: live SQL queries on a disposable CI PG database (never production).
use tor_reading::{repositories::PgReader, services::PublicReadingRepository};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci database"]
async fn public_chapters_hide_drafts_embargoes_admin_holds_paid_and_private_books() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let reader = PgReader::connect(&url).await.expect("restricted PG reader");
    let public = reader.public_book(1).await.unwrap().unwrap();
    assert_eq!(public.title, "TOR CI public");
    assert_eq!(public.chapters.len(), 3, "2 free, 1 paid metadata only");
    assert_eq!(public.chapters.iter().filter(|c| !c.is_paid).count(), 2);
    assert!(public.chapters.iter().any(|c| c.is_paid && c.id == 103));
    let paid = reader
        .public_paid_chapter_preview(103)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paid.book_id, 1);
    assert_eq!(paid.chapter_no, 3);
    assert!(paid.price_minor > 0);
    // A paywall query never permits looking up a hidden or unreleased chapter.
    for hidden in [0, 102, 104, 105, 106, 107, u64::MAX] {
        assert!(
            reader
                .public_paid_chapter_preview(hidden)
                .await
                .unwrap()
                .is_none(),
            "no public paid preview for chapter {hidden}"
        );
    }
    assert!(!public
        .chapters
        .iter()
        .any(|c| c.id == 102 || c.id == 106 || c.id == 107));
    let first = reader.public_chapter(101).await.unwrap().unwrap();
    assert!(first.body_markdown.contains("Первая строка"));
    assert_eq!(first.next_chapter_id, Some(103));
    assert!(
        first.next_chapter_is_paid,
        "nearest published chapter is paid, so it must lead to the paywall"
    );
    assert_eq!(first.previous_chapter_id, None);
    let last = reader.public_chapter(108).await.unwrap().unwrap();
    assert_eq!(last.previous_chapter_id, Some(101));
    assert_eq!(last.next_chapter_id, None);
    assert!(!last.next_chapter_is_paid);
    for hidden in [0, 102, 103, 104, 105, 106, 107, u64::MAX] {
        assert!(
            reader.public_chapter(hidden).await.unwrap().is_none(),
            "hidden id {hidden}"
        );
    }
    for hidden_book in [2, 3, u64::MAX] {
        assert!(reader.public_book(hidden_book).await.unwrap().is_none());
    }
    assert!(reader.probe().await.is_ok());
}

/// C-03 / S005: 501 real PostgreSQL chapters; no OFFSET, gap, duplication or hidden rows.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci database"]
async fn toc_501_chapters_keyset_pages_without_gaps() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let pool = sqlx::PgPool::connect(&url).await.expect("tor_app pool");
    let reader = PgReader::new(pool.clone());
    let book_id = 9_000_501_i64;
    sqlx::query(
        "INSERT INTO books (id, source_identity, target_language, title, visibility, access_level) \
         VALUES ($1, 'ci:toc:501', 'ru', 'Длинная книга', 'public', 'basic')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters (id, book_id, chapter_no, title, visibility, access_level, price_amount) \
         SELECT 8000000 + n, $1, n, 'Глава ' || n, 'public', \
                CASE WHEN n = 250 THEN 'paid' ELSE 'free' END, \
                CASE WHEN n = 250 THEN 100 ELSE 0 END \
         FROM generate_series(1, 501) AS series(n)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions (id, chapter_id, version_no, body_markdown) \
         SELECT 9000000 + n, 8000000 + n, 1, \
                CASE WHEN n = 250 THEN 'PRIVATE-PAID-TEXT' ELSE 'free text' END \
         FROM generate_series(1, 501) AS series(n)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications (chapter_id, version_id) \
         SELECT 8000000 + n, 9000000 + n \
         FROM generate_series(1, 501) AS series(n)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters (id, book_id, chapter_no, title, visibility, access_level, price_amount) \
         VALUES (8000502, $1, 502, 'HIDDEN-SENTINEL', 'draft', 'free', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions (id, chapter_id, version_no, body_markdown) \
         VALUES (9000502, 8000502, 1, 'PRIVATE-DRAFT-TEXT')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications (chapter_id, version_id) VALUES (8000502, 9000502)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut cursor = None;
    let mut numbers = Vec::new();
    let mut page_count = 0;
    loop {
        let page = reader
            .public_book_page(book_id as u64, 100, cursor.as_deref())
            .await
            .unwrap()
            .unwrap();
        page_count += 1;
        assert!(page.chapters.len() <= 100);
        numbers.extend(page.chapters.iter().map(|chapter| chapter.number));
        assert!(!page
            .chapters
            .iter()
            .any(|chapter| chapter.title == "HIDDEN-SENTINEL"));
        if let Some(next) = page.next_cursor {
            assert_ne!(cursor.as_deref(), Some(next.as_str()));
            cursor = Some(next);
        } else {
            break;
        }
        assert!(page_count <= 6, "cursor failed to advance");
    }
    assert_eq!(page_count, 6);
    assert_eq!(numbers, (1..=501).collect::<Vec<i32>>());
    assert!(reader
        .public_book_page(2, 100, None)
        .await
        .unwrap()
        .is_none());
    for invalid in [Some("0.1"), Some("1.0"), Some("1.2.3"), Some("-5.8")] {
        assert_eq!(
            reader
                .public_book_page(book_id as u64, 100, invalid)
                .await
                .err(),
            Some(tor_reading::services::ReadingError::InvalidPage)
        );
    }
    assert_eq!(
        reader
            .public_book_page(book_id as u64, 101, None)
            .await
            .err(),
        Some(tor_reading::services::ReadingError::InvalidPage)
    );
    // C-03/H: open in the middle without fetching the first 249 chapters.
    use tor_reading::services::toc::{TocCursor, TocWindowPosition};
    let around = reader
        .public_toc_window_for_session(
            book_id as u64,
            TocWindowPosition::Around(8_000_250),
            99,
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(around.total_chapters, Some(501));
    assert_eq!(around.chapters.len(), 99);
    assert_eq!(around.chapters.first().unwrap().number, 201);
    assert_eq!(around.chapters.last().unwrap().number, 299);
    assert!(around
        .chapters
        .iter()
        .any(|chapter| chapter.id == 8_000_250 && chapter.is_paid));
    assert_eq!(around.before_cursor.as_deref(), Some("201.8000201"));
    assert_eq!(around.after_cursor.as_deref(), Some("299.8000299"));

    let mut collected = around
        .chapters
        .iter()
        .map(|chapter| chapter.number)
        .collect::<Vec<_>>();
    let mut before = around.before_cursor.clone();
    while let Some(raw) = before.take() {
        let page = reader
            .public_toc_window_for_session(
                book_id as u64,
                TocWindowPosition::Before(TocCursor::parse(&raw).unwrap()),
                60,
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(page.chapters.len() <= 60);
        let mut older = page
            .chapters
            .iter()
            .map(|chapter| chapter.number)
            .collect::<Vec<_>>();
        older.append(&mut collected);
        collected = older;
        before = page.before_cursor;
    }
    let mut after = around.after_cursor.clone();
    while let Some(raw) = after.take() {
        let page = reader
            .public_toc_window_for_session(
                book_id as u64,
                TocWindowPosition::After(TocCursor::parse(&raw).unwrap()),
                60,
                None,
            )
            .await
            .unwrap()
            .unwrap();
        collected.extend(page.chapters.iter().map(|chapter| chapter.number));
        after = page.after_cursor;
    }
    assert_eq!(collected, (1..=501).collect::<Vec<i32>>());
    for hidden in [8_000_502, 999_999_999] {
        assert!(reader
            .public_toc_window_for_session(
                book_id as u64,
                TocWindowPosition::Around(hidden),
                99,
                None,
            )
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(
        reader
            .public_toc_window_for_session(
                book_id as u64,
                TocWindowPosition::Around(8_000_250),
                101,
                None,
            )
            .await
            .err(),
        Some(tor_reading::services::ReadingError::InvalidPage)
    );
    assert_eq!(
        reader
            .public_toc_window_for_session(
                book_id as u64,
                TocWindowPosition::After(TocCursor::parse("249.8000250").unwrap()),
                60,
                None,
            )
            .await
            .err(),
        Some(tor_reading::services::ReadingError::InvalidPage),
        "a forged number/id cursor cannot select unrelated chapters"
    );
    sqlx::query("DELETE FROM books WHERE id = $1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
}
