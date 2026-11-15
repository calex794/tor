//! Author book and chapter UI regression cases, including long-book navigation.
#[test]
fn s020_age_settings_link_is_hidden_until_owner_editor_is_enabled() {
    let book = OwnedBook {
        id: 42,
        title: "Проверка возраста".to_owned(),
    };
    let page = |age_editor_enabled| {
        detail_with_covers_with_demand_page(
            &book,
            false,
            &[],
            "csrf",
            OwnerChapterPageOptions {
                uploads_enabled: false,
                latest_job: None,
                demand_count: 0,
                paid_editor_enabled: false,
                age_editor_enabled,
                next: None,
                has_previous_page: false,
            },
        )
        .0
    };
    let disabled = page(false);
    assert!(!disabled.contains("/settings/access-level"));
    let enabled = page(true);
    assert!(enabled.contains("href=\"/me/books/42/settings/access-level\""));
    assert!(enabled.contains("Возрастной уровень книги"));
}

use super::*;

#[test]
fn book_workspace_keeps_release_and_paid_edit_gates_and_parent_navigation() {
    let book = OwnedBook {
        id: 42,
        title: "<bad>".into(),
    };
    let chapters = [
        OwnedChapterSummary {
            id: 11,
            number: 1,
            title: "<free>".into(),
            access: ChapterAccess::Free,
            published: false,
        },
        OwnedChapterSummary {
            id: 12,
            number: 2,
            title: "Paid V1".into(),
            access: ChapterAccess::Paid,
            published: true,
        },
        OwnedChapterSummary {
            id: 13,
            number: 3,
            title: "Paid draft".into(),
            access: ChapterAccess::Paid,
            published: false,
        },
    ];
    let page = |paid_editor_enabled| {
        detail_with_covers_with_demand_page(
            &book,
            true,
            &chapters,
            "<csrf>",
            OwnerChapterPageOptions {
                uploads_enabled: false,
                latest_job: None,
                demand_count: 0,
                paid_editor_enabled,
                age_editor_enabled: false,
                next: None,
                has_previous_page: false,
            },
        )
        .0
    };
    let disabled = page(false);
    assert!(disabled.contains("href=\"/me/books\" aria-current=\"location\""));
    assert!(!disabled.contains("aria-current=\"page\""));
    assert!(disabled.contains("/chapters/11/edit"));
    assert!(disabled.contains("/chapters/11/release"));
    assert!(!disabled.contains("/chapters/12/edit"));
    assert!(!disabled.contains("/chapters/13/edit"));
    assert!(!disabled.contains("/chapters/13/release"));
    assert!(disabled.contains("&lt;free&gt;"));
    assert!(disabled.contains("Публикация включена"));
    assert!(disabled.contains("проверки регистрации"));
    let enabled = page(true);
    assert!(enabled.contains("/chapters/12/edit"));
    assert!(!enabled.contains("/chapters/13/edit"));
}

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
            paid_editor_enabled: false,
            age_editor_enabled: false,
            next: Some(OwnedChapterCursor {
                number: 501,
                id: 501,
            }),
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
    let disabled = new_chapter(42, "csrf", "command", false, false, 1).0;
    assert!(!disabled.contains("name=\"is_delayed_publication\""));
    assert!(!disabled.contains("<option value=\"paid\">"));
    assert!(disabled.contains("name=\"access\" value=\"free\""));
    assert!(disabled.contains("value=\"1\" readonly required"));
    let enabled = new_chapter(42, "csrf", "command", true, true, 7).0;
    assert!(enabled.contains("name=\"is_delayed_publication\""));
    assert!(!enabled.contains("name=\"save_as_draft\""));
    assert!(enabled.contains("value=\"7\" readonly required"));
    assert!(enabled.contains("name=\"auto_number\" value=\"1\""));
    let free_public = new_chapter(42, "csrf", "command", false, true, 3).0;
    assert!(free_public.contains("name=\"is_delayed_publication\""));
    assert!(free_public.contains("value=\"3\" readonly required"));
    assert!(!free_public.contains("<option value=\"paid\">"));
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
    assert!(ready.contains("ожидает модератора"));
    let approved = cover_settings(&book, "secret", Some("moderation_approved"), true).0;
    assert!(approved.contains("Обложка одобрена модератором."));
    let rejected = cover_settings(&book, "secret", Some("moderation_rejected"), true).0;
    assert!(rejected.contains("Модератор отклонил обложку."));
    assert!(rejected.contains("src=\"/covers/42/main.webp\""));
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
    let page = release_chapter(2, 5, &chapter, "token", None);
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
    let detail_page = detail_with_covers(&published_book, true, &[summary], "csrf", false, None).0;
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

#[test]
fn author_book_draft_guidance_does_not_invent_metadata_readiness() {
    let book = OwnedBook {
        id: 42,
        title: "<safe title>".to_owned(),
    };
    let html = detail_with_covers_with_demand_page(
        &book,
        false,
        &[],
        "csrf",
        OwnerChapterPageOptions {
            uploads_enabled: false,
            latest_job: Some("moderation_pending"),
            demand_count: 0,
            paid_editor_enabled: false,
            age_editor_enabled: false,
            next: None,
            has_previous_page: false,
        },
    )
    .0;
    assert!(html.contains("Подготовка к публикации"));
    assert!(html.contains("Добавьте хотя бы одну бесплатную главу"));
    assert!(html.contains("Проверьте перед публикацией"));
    assert_eq!(html.matches(">Редактировать аннотацию</a>").count(), 1);
    assert!(html.contains(">Проверить описание</a>"));
    assert!(html.contains(">Создать главу</a>"));
    assert!(!html.contains("action=\"/me/books/42/publish\""));
    assert!(!html.contains("src=\"/covers/42/main.webp\""));
    assert!(html.contains("author-book-workspace__cover-placeholder"));
    assert!(html.contains("href=\"/me/books\" aria-current=\"location\""));
    assert!(html.contains("&lt;safe title&gt;"));
    assert!(!html.contains("<safe title>"));
}

#[test]
fn author_book_publishing_keeps_csrf_and_grouped_settings() {
    let book = OwnedBook {
        id: 42,
        title: "Роман".to_owned(),
    };
    let chapter = OwnedChapterSummary {
        id: 7,
        number: 1,
        title: "Первая".to_owned(),
        access: ChapterAccess::Free,
        published: false,
    };
    let html = detail_with_covers_with_demand_page(
        &book,
        false,
        &[chapter],
        "<csrf>",
        OwnerChapterPageOptions {
            uploads_enabled: true,
            latest_job: Some("moderation_approved"),
            demand_count: 0,
            paid_editor_enabled: false,
            age_editor_enabled: false,
            next: None,
            has_previous_page: false,
        },
    )
    .0;
    assert!(html.contains("method=\"post\" action=\"/me/books/42/publish\""));
    assert!(html.contains("name=\"csrf_token\" value=\"&lt;csrf&gt;\""));
    assert!(html.contains("src=\"/covers/42/main.webp\""));
    assert!(html.contains("Бесплатная глава есть на этой странице"));
    assert!(html.contains("Описание и оформление"));
    assert!(html.contains("Публикация и доступ"));
    assert!(html.contains("Статистика и обратная связь"));
    assert!(html.contains("id=\"author-book-chapters\""));
    assert!(html.contains("id=\"author-book-settings\""));
}

#[test]
fn author_book_published_state_does_not_show_draft_checklist() {
    let book = OwnedBook {
        id: 42,
        title: "Книга".to_owned(),
    };
    let html = detail_with_covers_with_demand_page(
        &book,
        true,
        &[],
        "csrf",
        OwnerChapterPageOptions {
            uploads_enabled: false,
            latest_job: None,
            demand_count: 5,
            paid_editor_enabled: false,
            age_editor_enabled: false,
            next: None,
            has_previous_page: false,
        },
    )
    .0;
    assert!(html.contains("Публикация включена"));
    assert!(!html.contains("Подготовка к публикации"));
    assert!(!html.contains("href=\"/me/books/42/chapters/import\""));
    assert!(html.contains("Читатели ждут продолжения"));
    assert!(html.contains("href=\"/books/42\""));
}

#[test]
fn owner_chapter_editors_use_shared_shell_and_keep_transaction_forms() {
    let draft = new_chapter(42, "<csrf>", "<command>", false, false, 3).0;
    assert!(draft.contains("class=\"account-sidebar\""));
    assert!(draft.contains("href=\"/me/books\" aria-current=\"location\""));
    assert!(!draft.contains("aria-current=\"page\""));
    assert!(draft.contains("author-editor-page__breadcrumb"));
    assert!(draft.contains("id=\"new-chapter-meta\""));
    assert!(draft.contains("id=\"new-chapter-text\""));
    assert!(draft.contains("action=\"/me/books/42/chapters\""));
    assert!(draft.contains("name=\"csrf_token\" value=\"&lt;csrf&gt;\""));
    assert!(draft.contains("name=\"command_key\" value=\"&lt;command&gt;\""));
    assert!(draft.contains("name=\"number\" type=\"number\""));
    assert!(draft.contains("name=\"auto_number\" value=\"1\""));
    assert!(draft.contains("name=\"access\" value=\"free\""));
    assert!(!draft.contains("name=\"is_delayed_publication\""));
    assert!(!draft.contains("<!--ACCOUNT_NAVIGATION-->"));

    let public = new_chapter(42, "csrf", "command", true, true, 4).0;
    assert!(public.contains("name=\"is_delayed_publication\""));
    assert!(public.contains("<option value=\"paid\">"));
    assert!(public.contains("name=\"price_amount\""));
    assert!(public.contains("id=\"new-chapter-text\""));

    let chapter = OwnedChapter {
        number: 4,
        title: "Опубликованная глава".to_owned(),
        body: "</textarea><script>bad()</script>".to_owned(),
        access: ChapterAccess::Paid,
        price_amount: 100,
        revision: 6,
    };
    let edit = edit_published_paid_chapter(42, 83, &chapter, "csrf", "key").0;
    assert!(edit.contains("href=\"/me/books\" aria-current=\"location\""));
    assert!(edit.contains("id=\"edit-chapter-meta\""));
    assert!(edit.contains("id=\"edit-chapter-text\""));
    assert!(edit.contains("action=\"/me/books/42/chapters/83/edit\""));
    assert!(edit.contains("name=\"expected_revision\" value=\"6\""));
    assert!(edit.contains("name=\"title\" type=\"text\""));
    assert!(edit.contains("readonly required"));
    assert!(edit.contains("Цена неизменна: 100 копеек."));
    assert!(edit.contains("href=\"/reader/83\""));
    assert!(edit.contains("&lt;/textarea&gt;&lt;script&gt;bad()"));
    assert!(!edit.contains("<script>bad()"));
    assert!(!edit.contains("<!--ACCOUNT_NAVIGATION-->"));
}
