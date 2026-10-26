
use super::*;
#[test]
fn escapes_untrusted_book_titles() {
    let page = home(&[PublicBookCard {
        id: 9,
        title: "<script>alert('x')</script> &".to_owned(),
        language: "ru".into(),
        author_name: None,
        genres: Vec::new(),
    }]);
    assert!(page.0.contains("&lt;script&gt;"));
    assert!(!page.0.contains("<script>"));
    assert!(page.0.contains("href=\"/books/9\""));
}
#[test]
fn catalog_metadata_must_be_escaped_in_cards() {
    let html = catalog_search(
        &[PublicBookCard {
            id: 17,
            title: "Visible".to_owned(),
            language: "ru".to_owned(),
            author_name: Some("<img src=x onerror=alert(1)>".to_owned()),
            genres: vec!["фэнтези<script>".to_owned()],
        }],
        "",
        None,
        Some("фэнтези"),
        None,
        None,
        None,
        "oldest",
        Some(1),
        None,
    )
    .0;
    assert!(html.contains("Найдено книг: 1"));
    assert!(html.contains("&lt;img"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<img src"));
    assert!(!html.contains("<script>"));
}

#[test]
fn toc_cursor_is_escaped_in_html_attribute() {
    let book = tor_reading::services::PublicBookPage {
        id: 7,
        title: "Test".into(),
        language: "ru".into(),
        chapters: vec![],
        next_cursor: Some("\" onclick=\"alert(1)".into()),
    };
    let html = book_detail_with_chapters(&book, 20).0;
    assert!(html.contains("after=&quot; onclick=&quot;alert(1)"));
    assert!(!html.contains("after=\" onclick=\""));
}
#[test]
fn empty_catalog_is_explicit() {
    assert!(catalog(&[])
        .0
        .contains("В каталоге пока нет опубликованных книг"));
}

#[test]
fn registration_has_russian_server_error_not_native_browser_popup() {
    let page = registration_form_with_error(
        "csrf-example",
        "<script>ошибка</script>",
        "reader\"@example.test",
        Some("Пароль должен содержать от 12 до 128 символов"),
    )
    .0;
    assert!(page.contains("lang=\"ru\""));
    assert!(page.contains("action=\"/register\" autocomplete=\"on\" novalidate"));
    assert!(page.contains("role=\"alert\""));
    assert!(page.contains("Пароль должен содержать от 12 до 128 символов"));
    assert!(page.contains("&lt;script&gt;ошибка&lt;/script&gt;"));
    assert!(page.contains("reader&quot;@example.test"));
    assert!(!page.contains("<script>ошибка</script>"));
    assert!(!page.contains("value=\"1234\""));
    assert!(registration_form("csrf-example").0.contains("novalidate"));
    assert!(!registration_form("csrf-example")
        .0
        .contains("<!--FORM_ERROR-->"));
    assert!(login_form("csrf-example").0.contains("novalidate"));
}

#[test]
fn account_failure_is_complete_branded_page_without_credentials() {
    let html = account_error_page(
        "Не удалось завершить регистрацию",
        "Проверьте данные и попробуйте снова.",
        true,
    )
    .0;
    assert!(html.contains("<html lang=\"ru\">"));
    assert!(html.contains("account-error__card"));
    assert!(html.contains("href=\"/register\""));
    assert!(html.contains("href=\"/login\""));
    assert!(html.contains("href=\"/catalog\""));
    assert!(html.contains("Не удалось завершить регистрацию"));
}

#[test]
fn profile_shows_only_actual_user_features() {
    let html = my_account("<reader>", "csrf").0;
    assert!(html.contains("&lt;reader&gt;"));
    assert!(html.contains("href=\"/me/books\""));
    assert!(html.contains("href=\"/me/library\""));
    assert!(!html.contains("Приватность"));
    assert!(!html.contains("Анализ ваших переходов"));
    assert!(!html.contains("/me/behavior"));
    assert!(!html.contains("RIL_BEHAVIOR_CONTROLS"));
}

#[test]
fn ne_wboo_public_shell_and_catalog_reuse_source_components() {
    let html = catalog(&[PublicBookCard {
        id: 4,
        title: "Старый <каталог>".into(),
        language: "ru".into(),
        author_name: Some("Автор".into()),
        genres: vec!["Фэнтези".into()],
    }])
    .0;
    assert!(html.contains("site-logo"));
    assert!(html.contains("href=\"/assets/newboo.css\""));
    assert!(html.contains("book-mini-card book-mini-card--compact"));
    assert!(html.contains("Старый &lt;каталог&gt;"));
    assert!(!html.contains("Старый <каталог>"));
    assert!(html.contains("href=\"/books/4\""));
    assert!(html.contains("Автор: Автор"));
    assert!(html.contains("Жанры: Фэнтези"));
    assert!(html.contains("Нижняя навигация"));
    assert!(!html.contains("Выберите свою следующую историю"));
}

#[test]
fn source_brand_is_exactly_a_png() {
    assert!(include_bytes!("../../assets/brand/pandaolate-header.png").starts_with(b"\x89PNG"));
}

#[test]
fn ne_wboo_book_detail_uses_original_hero_without_fake_cover_or_leaked_html() {
    let page = book_detail(&PublicBookCard {
        id: 15,
        title: "<script>Книга</script>".into(),
        language: "ru".into(),
        author_name: Some("<Автор>".into()),
        genres: vec!["Фэнтези".into()],
    })
    .0;
    assert!(page.contains("book-hero-card"));
    assert!(page.contains("book-cover-xl"));
    assert!(page.contains("Нет обложки"));
    assert!(page.contains("&lt;script&gt;Книга&lt;/script&gt;"));
    assert!(!page.contains("<script>Книга</script>"));
    assert!(page.contains("&lt;Автор&gt;"));
    assert!(page.contains("Фэнтези"));
    assert!(page.contains("href=\"/catalog\""));
    assert!(!page.contains("href=\"/reader/"));
    assert!(!page.contains("href=\"/me/library/add/15\""));
    assert!(!page.contains("<!--BOOK_CREDITS-->"));
    assert!(include_str!("../../assets/newboo-book-detail.css").contains(".book-hero-card"));
}

#[test]
fn book_detail_toc_keeps_approved_library_action_and_free_only_policy() {
    let book = tor_reading::services::PublicBookPage {
        id: 7,
        title: "Публичная".into(),
        language: "ru".into(),
        chapters: vec![],
        next_cursor: None,
    };
    let html = book_detail_with_chapters(&book, 20).0;
    assert!(html.contains("book-section-card chapter-toc"));
    assert!(html.contains("id=\"chapters\""));
    assert!(html.contains("href=\"#chapters\""));
    assert!(html.contains("href=\"/me/library/add/7\""));
    assert!(!html.contains("href=\"/reader/"));
    assert!(!html.contains("<!--CHAPTERS-->"));
}

#[test]
fn ne_wboo_reader_shell_uses_live_chapter_data_and_no_fake_controls() {
    let template = include_str!("../../templates/reader.html");
    assert!(template.contains("reader-topbar"));
    assert!(template.contains("reader-page__title"));
    assert!(template.contains("<!--CHAPTER_CONTENT-->"));
    assert!(template.contains("href=\"/books/<!--BOOK_ID-->\""));
    assert!(!template.contains("data-reader-theme-cycle"));
    assert!(!template.contains("Сохранение прогресса появится"));
    assert!(include_str!("../../assets/newboo-reader.css").contains(".reader-topbar__back"));
}
