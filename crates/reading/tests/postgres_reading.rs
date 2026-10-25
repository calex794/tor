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
    assert!(!public
        .chapters
        .iter()
        .any(|c| c.id == 102 || c.id == 106 || c.id == 107));
    let first = reader.public_chapter(101).await.unwrap().unwrap();
    assert!(first.body_markdown.contains("Первая строка"));
    assert_eq!(first.next_chapter_id, Some(108));
    assert_eq!(first.previous_chapter_id, None);
    let last = reader.public_chapter(108).await.unwrap().unwrap();
    assert_eq!(last.previous_chapter_id, Some(101));
    assert_eq!(last.next_chapter_id, None);
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
