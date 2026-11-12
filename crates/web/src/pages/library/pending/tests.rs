use super::*;
use tor_reading::{
    repositories::follow_updates::FollowedReleasePage,
    services::library::{SavedBook, SavedBookPage},
};

#[test]
fn pending_deletion_book_is_private_visible_and_distinct_from_deleted() {
    let saved = SavedBookPage {
        items: vec![
            SavedBook {
                cursor_id: 22,
                book_id: 55,
                title: "Тест <script>alert(1)</script>".into(),
                pending_deletion: true,
                ordinary_deleted: false,
            },
            SavedBook {
                cursor_id: 21,
                book_id: 56,
                title: "Обычная книга".into(),
                pending_deletion: false,
                ordinary_deleted: false,
            },
        ],
        next_cursor: None,
    };
    let updates = FollowedReleasePage {
        items: Vec::new(),
        next_cursor: None,
    };
    let page = my_library_with_updates_and_layers_and_purchased_only_selected(
        &[],
        &saved,
        "csrf-token",
        (false, false),
        false,
        (&updates, &[]),
        (&tor_reading::services::library::PurchasedOnlyBookPage {
            items: vec![],
            next_cursor: None,
        }, Some("saved")),
    );
    let html = page.0;
    assert!(html.contains("Ожидает удаления"));
    assert_eq!(html.matches("Ожидает удаления").count(), 1);
    assert!(!html.contains(">Удалена<"));
    assert!(html.contains("href=\"/books/55\""));
    assert!(html.contains("action=\"/me/library/55/remove\""));
    assert!(html.contains("href=\"/me/library/55/saved\""));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>alert(1)</script>"));
}
