use super::*;
use tor_reading::{
    repositories::follow_updates::FollowedReleasePage,
    services::{
        library::{PurchasedOnlyBookPage, SavedBookPage},
        progress::ContinueReading,
    },
};

fn blank_saved() -> SavedBookPage {
    SavedBookPage {
        items: vec![],
        next_cursor: None,
    }
}
fn blank_updates() -> FollowedReleasePage {
    FollowedReleasePage {
        items: vec![],
        next_cursor: None,
    }
}
fn blank_purchased() -> PurchasedOnlyBookPage {
    PurchasedOnlyBookPage {
        items: vec![],
        next_cursor: None,
    }
}

#[test]
fn empty_library_has_one_onboarding_state_and_csp_safe_navigation() {
    let updates = blank_updates();
    let purchases = blank_purchased();
    let html = my_library_with_updates_and_layers_and_purchased_only_selected(
        &[],
        &blank_saved(),
        "csrf",
        (false, false),
        false,
        (&updates, &[]),
        (&purchases, None),
    )
    .0;
    assert_eq!(html.matches("Ваша книжная полка пока пуста").count(), 1);
    assert!(!html.contains("После подписки новые главы появятся здесь"));
    assert!(html.contains("href=\"/me/library?view=saved\""));
    assert!(html.contains("aria-current=\"page\""));
    assert!(!html.contains("<script"));
}

#[test]
fn selected_panels_preserve_private_archive_link_and_cover_boundary() {
    let updates = blank_updates();
    let purchases = blank_purchased();
    let items = [ContinueReading {
        book_id: 9,
        book_title: "Архив <hidden>".to_owned(),
        chapter_id: 23,
        chapter_no: 5,
        chapter_title: "Старая редакция".to_owned(),
        block_index: 3,
        archived_saved: true,
    }];
    let html = my_library_with_updates_and_layers_and_purchased_only_selected(
        &items,
        &blank_saved(),
        "csrf",
        (false, false),
        false,
        (&updates, &[]),
        (&purchases, Some("reading")),
    )
    .0;
    assert!(html.contains("href=\"/me/library/saved/23#saved-resume\""));
    assert!(html.contains("&lt;hidden&gt;"));
    assert!(!html.contains("href=\"/covers/9/card.webp\""));
    let saved_html = my_library_with_updates_and_layers_and_purchased_only_selected(
        &items,
        &blank_saved(),
        "csrf",
        (false, false),
        false,
        (&updates, &[]),
        (&purchases, Some("saved")),
    )
    .0;
    assert!(saved_html.contains("id=\"saved-books\""));
    assert!(!saved_html.contains("href=\"/me/library/saved/23#saved-resume\""));
}

#[test]
fn public_library_cover_keeps_script_free_visual_fallback() {
    let with_cover = book_art(13, true);
    assert!(with_cover.contains("class=\"library-book-art__cover\""));
    assert!(with_cover.contains("<image href=\"/covers/13/card.webp\""));
    assert!(with_cover.contains("library-book-art__glyph"));
    assert!(!with_cover.contains("<img"));
    assert!(!with_cover.contains("onerror="));

    let private = book_art(13, false);
    assert!(!private.contains("/covers/13/card.webp"));
    assert!(private.contains("library-book-art__glyph"));
}
