//! Author book and chapter UI regression cases, including long-book navigation.
use super::*;

#[test]
fn b12_owner_pages_keep_navigation_for_long_books() {
    let book = OwnedBook {
        id: 42,
        title: "Повесть".to_owned(),
    };
    let chapter = OwnedChapterSummary {
        id: 501,
        number: 501,
        title: "<безопасная>".to_owned(),
        access: ChapterAccess::Free,
        published: true,
    };
    let page = detail_with_covers_with_demand_page(
        &book,
        true,
        &[chapter],
        "csrf",
        OwnerChapterPageOptions {
            uploads_enabled: false,
            latest_job: None,
            demand_count: 0,
            next: Some(OwnedChapterCursor { number: 501, id: 501 }),
            has_previous_page: true,
        },
    )
    .0;
    assert!(page.contains("/me/books/42?after=501:501"));
    assert!(page.contains("К первым главам"));
    assert!(page.contains("&lt;безопасная&gt;"));
    assert!(!page.contains("Показаны первые 200 глав"));
}
#[test]
fn paid_controls_only_appear_in_enabled_author_pages() {
    let disabled = new_chapter(42, "csrf", "command", false).0;
    assert!(!disabled.contains("<option value=\"paid\">"));
    assert!(disabled.contains("name=\"access\" value=\"free\""));
    let enabled = new_chapter(42, "csrf", "command", true).0;
    assert!(enabled.contains("<option value=\"paid\">"));
    assert!(enabled.contains("name=\"price_amount\""));
    assert!(enabled.contains("сразу после сохранения"));
    assert!(!enabled.contains("<!--CHAPTER_ACCESS-->"));
    assert!(enabled.contains("/me/books/42/chapters/illustrated"));
    assert!(!disabled.contains("/me/books/42/chapters/illustrated"));
    let illustrated = illustrated_chapter(42, "csrf", "command").0;
    assert!(illustrated.contains("enctype=\"multipart/form-data\""));
    assert!(illustrated.contains("name=\"illustration\""));
    assert!(illustrated.contains("name=\"csrf_token\" value=\"csrf\""));
    assert!(!illustrated.contains("<script"));
}

#[test]
fn author_title_is_always_escaped() {
    let page = detail(
        &OwnedBook {
            id: 42,
            title: "<img src=x onerror=1>".to_owned(),
        },
        false,
        &[],
        "test-token",
    );
    assert!(page.0.contains("&lt;img"));
    assert!(!page.0.contains("<img src=x"));
}
#[test]
fn uploaded_cover_form_requires_csrf_and_never_claims_published_before_done() {
    let book = OwnedBook {
        id: 42,
        title: "Книга".to_owned(),
    };
    let disabled = detail(&book, false, &[], "secret").0;
    assert!(!disabled.contains("enctype=\"multipart/form-data\""));
    let waiting = detail_with_covers(&book, false, &[], "secret", true, Some("pending")).0;
    assert!(waiting.contains("href=\"/me/books/42/settings/covers\""));
    assert!(!waiting.contains("enctype=\"multipart/form-data\""));
    assert!(waiting.contains("ожидает обработки"));
    let settings = cover_settings(&book, "secret", Some("pending"), false).0;
    assert!(settings.contains("action=\"/me/books/42/cover\""));
    assert!(settings.contains("name=\"csrf_token\" value=\"secret\""));
    assert!(settings.contains("book-cover-picker__frame"));
    assert!(settings.contains("data-cover-picker-input"));
    assert!(settings.contains("book-cover-picker__plus"));
    assert!(settings.contains("Обложка 1"));
    assert!(!settings.contains("/covers/42/main.webp"));
    let ready = cover_settings(&book, "secret", Some("done"), true).0;
    assert!(ready.contains("Обложка готова"));
    assert!(ready.contains("src=\"/covers/42/main.webp\""));
    assert!(ready.contains("Заменить"));
}

#[test]
fn release_form_escapes_chapter_body_and_pins_version() {
    let chapter = PendingChapter {
        number: 4,
        title: "<svg onload=1>".into(),
        body: "</pre><script>bad()</script>".into(),
        revision: 3,
        version_id: 86,
    };
    let page = release_chapter(2, 5, &chapter, "token");
    assert!(page.0.contains("name=\"version_id\" value=\"86\""));
    assert!(page.0.contains("name=\"expected_revision\" value=\"3\""));
    assert!(page.0.contains("&lt;/pre&gt;"));
    assert!(!page.0.contains("<script>bad()"));
}
#[test]
fn published_free_chapter_has_edit_link_and_immediate_release_copy() {
    let published_book = OwnedBook {
        id: 42,
        title: "Книга".to_owned(),
    };
    let summary = OwnedChapterSummary {
        id: 55,
        number: 1,
        title: "Редакция".to_owned(),
        access: ChapterAccess::Free,
        published: true,
    };
    let detail_page =
        detail_with_covers(&published_book, true, &[summary], "csrf", false, None).0;
    assert!(detail_page.contains("/me/books/42/chapters/55/edit"));
    let chapter = OwnedChapter {
        number: 1,
        title: "Новая глава".to_owned(),
        body: "Содержание".to_owned(),
        access: ChapterAccess::Free,
        price_amount: 0,
        revision: 3,
    };
    let page = edit_published_chapter(42, 55, &chapter, "csrf", "command").0;
    assert!(page.contains("Сохранение сразу обновит опубликованную редакцию"));
    assert!(page.contains("href=\"/reader/55\""));
    assert!(page.contains(">Новая глава</a>"));
    assert!(!page.contains("<!--TITLE_HEADING-->"));
    assert!(page.contains("name=\"expected_revision\" value=\"3\""));
    assert!(!page.contains("<!--EDIT_DESCRIPTION-->"));
}

#[test]
fn edit_form_escapes_title_and_textarea_breakout() {
    let chapter = OwnedChapter {
        number: 1,
        title: "<script>alert(1)</script>".to_owned(),
        body: "</textarea><script>evil()</script>".to_owned(),
        access: ChapterAccess::Free,
        price_amount: 0,
        revision: 7,
    };
    let page = edit_chapter(1, 3, &chapter, "token", "key");
    assert!(page.0.contains("value=\"7\""));
    assert!(page.0.contains("&lt;/textarea&gt;"));
    assert!(!page.0.contains("<script>evil()"));
    assert!(!page.0.contains("href=\"/reader/3\""));
    assert!(page.0.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!page.0.contains("<!--TITLE_HEADING-->"));
}
