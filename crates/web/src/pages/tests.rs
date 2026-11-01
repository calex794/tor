use super::*;

#[test]
fn s035_book_detail_only_links_batch_when_enabled_and_two_paid_chapters_exist() {
    let book = tor_reading::services::PublicBookPage {
        id: 91,
        title: "<script>Book</script>".to_owned(),
        language: "ru".to_owned(),
        author_name: String::new(),
        genres: Vec::new(),
        description: String::new(),
        chapters: (1_u64..=2)
            .map(|id| tor_reading::services::PublicChapterLink {
                id,
                number: id as i32,
                title: "Платная".to_owned(),
                is_paid: true,
                price_minor: 100,
            })
            .collect(),
        next_cursor: None,
    };
    let hidden = book_detail_with_chapters_cover_internal(&book, 20, false, false).0;
    assert!(!hidden.contains("buy-chapters"));
    let enabled = book_detail_with_chapters_cover_internal(&book, 20, false, true).0;
    assert!(enabled.contains("href=\"/me/books/91/buy-chapters\""));
    assert!(!enabled.contains("<script>Book</script>"));
    assert!(enabled.contains("&lt;script&gt;Book&lt;/script&gt;"));
}

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
        CatalogPageQuery {
            query: "",
            language: None,
            genre: Some("фэнтези"),
            tag: None,
            fandom: None,
            cursor: None,
            previous_cursor: None,
            page_number: 1,
            sort: "oldest",
            total: Some(1),
            options: None,
        },
    )
    .0;
    assert!(html.contains("Найдено книг: 1"));
    assert!(html.contains("&lt;img"));
    assert!(!html.contains("фэнтези&lt;script&gt;"));
    assert!(!html.contains("<img src"));
    assert!(!html.contains("<script>"));
    assert!(!html.contains("Жанры:"));
}

#[test]
fn toc_cursor_is_escaped_in_html_attribute() {
    let book = tor_reading::services::PublicBookPage {
        id: 7,
        title: "Test".into(),
        language: "ru".into(),
        author_name: String::new(),
        genres: Vec::new(),
        description: String::new(),
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
fn auth_forms_preserve_explicit_private_reader_return() {
    let login = login_form_with_next("csrf", Some("/read/42")).0;
    assert!(login.contains("name=\"next\" value=\"/read/42\""));
    assert!(login.contains("href=\"/register?next=/read/42\""));
    let register = registration_form_with_next("csrf", Some("/read/42")).0;
    assert!(register.contains("name=\"next\" value=\"/read/42\""));
    assert!(register.contains("href=\"/login?next=/read/42\""));
    let error = registration_form_with_error_and_next(
        "csrf",
        "Reader",
        "reader@example.test",
        Some("Повторите отправку"),
        Some("/read/42"),
    )
    .0;
    assert!(error.contains("name=\"next\" value=\"/read/42\""));
    assert!(!login_form("csrf").0.contains("name=\"next\""));
    assert!(!registration_form("csrf").0.contains("name=\"next\""));
    assert!(!login.contains("<!--AUTH_RETURN_FIELD-->"));
    assert!(!register.contains("<!--AUTH_LOGIN_NEXT-->"));
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
    let html = my_account("<reader>", "csrf", &[]).0;
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
    assert!(html.contains("<div class=\"book-mini-card__meta\">Автор</div>"));
    assert!(!html.contains("Жанры: Фэнтези"));
    assert!(!html.contains("Язык: ru"));
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
        author_name: String::new(),
        genres: Vec::new(),
        description: String::new(),
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
fn s005_paywall_escapes_public_metadata_and_never_fakes_checkout() {
    let page = paid_chapter_paywall(&tor_reading::services::PublicPaidChapterPreview {
        book_id: 99,
        book_title: "<script>Книга</script>".to_owned(),
        chapter_no: 7,
        chapter_title: "Глава \"<script>\"".to_owned(),
        price_minor: 1234,
    })
    .0;
    assert!(page.contains("Платная глава"));
    assert!(page.contains("&lt;script&gt;Книга&lt;/script&gt;"));
    assert!(page.contains("Глава &quot;&lt;script&gt;&quot;"));
    assert!(!page.contains("<script>Книга</script>"));
    assert!(page.contains("12,34 ₽"));
    assert!(page.contains("href=\"/login\""));
    assert!(page.contains("href=\"/books/99#chapters\""));
    assert!(!page.contains("api/v1/purchases"));
    assert!(!page.contains("content_access"));
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

#[test]
fn seo02_public_book_renders_real_author_genres_and_escaped_description() {
    let book = tor_reading::services::PublicBookPage {
        id: 77,
        title: "Рассказ".into(),
        language: "ru".into(),
        author_name: "<Автор>".into(),
        genres: vec!["Фэнтези".into()],
        description: "Книга о природе\n</p><script>alert(1)</script>".into(),
        chapters: Vec::new(),
        next_cursor: None,
    };
    let html = book_detail_with_chapters(&book, 20).0;
    assert!(html.contains("&lt;Автор&gt;"));
    assert!(html.contains("Фэнтези"));
    assert!(html.contains("Книга о природе<br>&lt;/p&gt;&lt;script&gt;"));
    assert!(!html.contains("</p><script>"));
    assert!(!html.contains("Карточка книги в каталоге Пандаолейт."));
}

#[test]
fn seo02_old_books_with_no_description_do_not_invent_plot() {
    let book = tor_reading::services::PublicBookPage {
        id: 78,
        title: "Старая книга".into(),
        language: "ru".into(),
        author_name: String::new(),
        genres: Vec::new(),
        description: String::new(),
        chapters: Vec::new(),
        next_cursor: None,
    };
    let html = book_detail_with_chapters(&book, 20).0;
    assert!(html.contains("Аннотация пока не добавлена."));
    assert!(!html.contains("Откройте оглавление, чтобы читать доступные главы бесплатно."));
}

#[test]
fn c_14_public_continuation_distinguishes_read_access_and_missing_release() {
    use tor_reading::services::{PublicChapterPage, PublicContinuation};
    let mut chapter = PublicChapterPage {
        version_id: 10,
        book_id: 20,
        book_title: "Книга <скрипт>".to_owned(),
        chapter_title: "Глава 1".to_owned(),
        version_no: 1,
        body_markdown: "Безопасный текст".to_owned(),
        previous_chapter_id: None,
        next_chapter_id: Some(31),
        next_chapter_is_paid: false,
    };
    assert_eq!(chapter.continuation(), PublicContinuation::ReadNow(31));
    let free = reader(&chapter).0;
    assert!(free.contains("href=\"/reader/31\""));
    assert!(free.contains("Следующая глава</a>"));
    assert!(!free.contains("пока нет"));

    chapter.next_chapter_is_paid = true;
    assert_eq!(chapter.continuation(), PublicContinuation::NeedsAccess(31));
    let paid = reader(&chapter).0;
    assert!(paid.contains("Следующая глава — платная"));
    assert!(paid.contains("может потребоваться покупка"));
    assert!(!paid.contains("Следующих опубликованных глав пока нет"));
    assert!(!paid.contains("Книга <скрипт>"));

    chapter.next_chapter_id = None;
    chapter.next_chapter_is_paid = false;
    assert_eq!(
        chapter.continuation(),
        PublicContinuation::WaitingForRelease
    );
    let waiting = reader(&chapter).0;
    assert!(waiting.contains("Следующих опубликованных глав пока нет"));
    assert!(!waiting.contains("href=\"/reader/31\""));
}

#[test]
fn c_15_public_caught_up_guides_reader_to_explicit_private_completion() {
    let mut chapter = tor_reading::services::PublicChapterPage {
        version_id: 15,
        book_id: 6,
        book_title: "Ожидаемая книга".to_owned(),
        chapter_title: "Конец главы".to_owned(),
        version_no: 1,
        body_markdown: "Последний абзац".to_owned(),
        previous_chapter_id: None,
        next_chapter_id: None,
        next_chapter_is_paid: false,
    };
    let waiting = reader_with_login_link(&chapter, Some(77)).0;
    assert!(waiting.contains("href=\"/read/77\""));
    assert!(waiting.contains("попросить продолжение"));
    assert!(!reader(&chapter).0.contains("href=\"/read/77\""));
    chapter.next_chapter_id = Some(78);
    chapter.next_chapter_is_paid = true;
    let published = reader_with_login_link(&chapter, Some(77)).0;
    assert!(!published.contains("href=\"/read/77\""));
    assert!(published.contains("Следующая глава — платная"));
    let private = private_reader(
        77,
        &chapter,
        "csrf",
        tor_reading::services::progress::SavedPlaceState::NotSaved,
        0,
        false,
    )
    .0;
    assert!(private.contains("href=\"/reader/78\""));
    assert!(!private.contains("href=\"/read/78\""));
}

#[test]
fn b_18_navigation_follows_books_in_catalog_layout() {
    let query = CatalogPageQuery {
        query: "",
        language: None,
        genre: None,
        tag: None,
        fandom: None,
        cursor: Some("20"),
        previous_cursor: None,
        page_number: 1,
        sort: "oldest",
        total: Some(42),
        options: None,
    };
    let html = catalog_search(
        &[PublicBookCard {
            id: 20,
            title: "Последняя на странице".into(),
            language: "ru".into(),
            author_name: None,
            genres: vec![],
        }],
        query,
    )
    .0;
    let card = html.find("Последняя на странице").expect("book card");
    let navigation = html.find("catalog-pagination").expect("page controls");
    assert!(navigation > card, "page navigation follows the book grid");
    assert!(html.contains("Страница 1"));
}
