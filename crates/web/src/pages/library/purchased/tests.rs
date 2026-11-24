use super::*;
use tor_reading::{
    repositories::follow_updates::FollowedReleasePage,
    services::library::{PurchasedOnlyBook, PurchasedOnlyBookPage, SavedBookPage},
};

#[test]
fn shared_book_art_never_requests_unconfirmed_or_nonpublic_cover() {
    let known = std::collections::HashSet::from([42_u64]);
    assert!(book_art(42, true, Some(&known)).contains("/covers/42/card.webp"));
    assert!(book_art(42, true, None).contains("/covers/42/card.webp"));
    for art in [
        book_art(42, false, Some(&known)),
        book_art(42, false, None),
        book_art(43, true, Some(&known)),
        book_art(0, true, None),
        book_art(-1, true, None),
    ] {
        assert!(!art.contains("<img"));
        assert!(art.contains("library-book-art__glyph"));
    }
}

#[test]
fn shared_deletion_notice_retains_precedence() {
    assert_eq!(deletion_notice(false, false), "");
    assert!(deletion_notice(false, true).contains("Ожидает удаления"));
    assert!(deletion_notice(true, false).contains(">Удалена<"));
    assert_eq!(deletion_notice(true, true), deletion_notice(true, false));
}

#[test]
fn bought_only_deleted_book_stays_in_library_without_remove_bookmark_action() {
    let bookmarked = SavedBookPage {
        items: vec![],
        next_cursor: None,
    };
    let only_bought = PurchasedOnlyBookPage {
        items: vec![
            PurchasedOnlyBook {
                book_id: 88,
                title: "Removed <script>alert(1)</script>".into(),
                pending_deletion: false,
                ordinary_deleted: true,
            },
            PurchasedOnlyBook {
                book_id: 77,
                title: "Pending".into(),
                pending_deletion: true,
                ordinary_deleted: false,
            },
        ],
        next_cursor: Some(77),
    };
    let updates = FollowedReleasePage {
        items: vec![],
        next_cursor: None,
    };
    let html = my_library_with_updates_and_layers_and_purchased_only_selected(
        &[],
        &bookmarked,
        "csrf",
        (true, false),
        false,
        (&updates, &[]),
        (&only_bought, Some("purchased"), None),
    )
    .0;
    assert!(html.contains("aria-label=\"Книги с покупками\""));
    assert!(html.contains("href=\"/me/library/88/saved\""));
    assert!(html.contains("href=\"/me/library/77/saved\""));
    assert!(html.contains("href=\"/me/library?purchased_after=77\""));
    assert!(html.contains("Удалена"));
    assert!(html.contains("Ожидает удаления"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(!html.contains("action=\"/me/library/88/remove\""));
    assert!(!html.contains("action=\"/me/library/77/remove\""));
    assert!(!html.contains("/covers/88/card.webp"));
    assert!(!html.contains("/covers/77/card.webp"));

    let unavailable = my_library_with_updates_and_layers_and_purchased_only_selected(
        &[],
        &bookmarked,
        "csrf",
        (false, false),
        false,
        (&updates, &[]),
        (&only_bought, Some("purchased"), None),
    )
    .0;
    assert!(unavailable.contains("чтение временно недоступно"));
    assert!(!unavailable.contains("href=\"/me/library/88/saved\""));
}
