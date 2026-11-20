//! UI-12: script-free book hero secondary actions contract.
use super::*;

#[test]
fn ui_12_book_secondary_actions_are_accessible_without_js() {
    let html = book_detail(&PublicBookCard {
        id: 15,
        title: "Книга".into(),
        language: "ru".into(),
        author_name: None,
        genres: vec![],
    })
    .0;
    let start = html
        .find("<details class=\"book-hero-more\">")
        .expect("script-free collapsed secondary control");
    let close = html[start..].find("</details>").expect("closed details");
    let menu = &html[start..start + close + "</details>".len()];
    assert!(menu.contains("<summary class=\"ui-button ui-button--ghost\">Ещё действия</summary>"));
    assert!(!menu.contains("<script"));
    assert_eq!(menu.matches("href=\"/me/referrals?book=15\"").count(), 1);
    assert_eq!(menu.matches("href=\"/books/15/share\"").count(), 1);
    assert_eq!(menu.matches("href=\"/books/15/report\"").count(), 1);
    assert!(html.find("<!--BOOK_FOLLOW_ACTION-->").unwrap() < start);
    assert!(html.find("<!--BOOK_OWNER_ACTIONS-->").unwrap() < start);
    assert!(!html.contains("<details class=\"book-hero-more\" open"));
    let css = include_str!("../../assets/newboo-book-detail.css");
    assert!(css.contains(".book-hero-more > summary:focus-visible"));
    assert!(css.contains(".book-hero-more[open]"));
}
