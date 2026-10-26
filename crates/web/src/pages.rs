//! Server-rendered demo pages based on BookaRust source d11b27b43125b93e59809147e8e687203fa0753b.
use axum::{
    http::header,
    response::{Html, IntoResponse},
};
use std::fmt::Write;

mod account;
pub use account::{
    account_error_page, login_form, my_account, registration_form, registration_form_with_error,
};
mod public_catalog;
pub use public_catalog::{catalog, catalog_search, home, PublicBookCard};

pub fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub fn layout(title: &str, description: &str, content: &str) -> Html<String> {
    Html(
        include_str!("../templates/layout.html")
            .replace("<!--PAGE_TITLE-->", &escape_html(title))
            .replace("<!--PAGE_DESCRIPTION-->", &escape_html(description))
            .replace("<!--PAGE_CONTENT-->", content),
    )
}

/// The S0 demo and the PostgreSQL book view share exactly one page shell.
pub fn book_detail(book: &PublicBookCard) -> Html<String> {
    book_detail_body(book, None)
}

/// Public TOC: paid metadata can be listed but paid bodies are never fetched.
pub fn book_detail_with_chapters(
    book: &tor_reading::services::PublicBookPage,
    limit: u16,
) -> Html<String> {
    let card = PublicBookCard {
        id: book.id,
        title: book.title.clone(),
        language: book.language.clone(),
        author_name: None,
        genres: Vec::new(),
    };
    let mut items = String::new();
    if book.chapters.is_empty() {
        items.push_str("<p class=\"empty-state\">Опубликованных глав пока нет.</p>");
    } else {
        items.push_str("<ol class=\"chapter-toc__list\">");
        for chapter in &book.chapters {
            let title = escape_html(&chapter.title);
            if chapter.is_paid {
                write!(
                    items,
                    "<li class=\"chapter-toc__item chapter-toc__item--paid\"><span>Глава {}. {title}</span><span class=\"chapter-toc__badge\">Платная · пока недоступна</span></li>",
                    chapter.number
                ).expect("writing String");
            } else {
                write!(
                    items,
                    "<li class=\"chapter-toc__item\"><a href=\"/reader/{}\">Глава {}. {title}</a><span class=\"chapter-toc__badge chapter-toc__badge--free\">Бесплатно</span></li>",
                    chapter.id, chapter.number
                ).expect("writing String");
            }
        }
        items.push_str("</ol>");
    }
    if let Some(cursor) = &book.next_cursor {
        write!(
            items,
            "<nav class=\"chapter-toc__more\" aria-label=\"Страницы оглавления\"><a class=\"button button--small\" href=\"/books/{}?limit={}&amp;after={}#chapters\">Следующие главы</a></nav>",
            book.id, limit, escape_html(cursor)
        )
        .expect("writing String");
    }
    items.push_str(&format!(
        "<p><a class=\"button button--small\" href=\"/me/library/add/{}\">Добавить в библиотеку</a></p>",
        book.id
    ));
    let section = format!(
        "<section class=\"section chapter-toc\" id=\"chapters\" aria-labelledby=\"toc-title\"><div class=\"container\"><p class=\"eyebrow\">Оглавление</p><h2 id=\"toc-title\">Главы книги</h2>{items}</div></section>"
    );
    book_detail_body(&card, Some(&section))
}

fn book_detail_body(book: &PublicBookCard, toc: Option<&str>) -> Html<String> {
    let title = escape_html(&book.title);
    let language = escape_html(&book.language);
    let description = if toc.is_some() {
        "Откройте оглавление, чтобы читать доступные главы бесплатно. Платные главы не открываются без проверки прав."
    } else {
        "Карточка опубликованной книги. Оглавление и чтение глав появятся после подключения читалки."
    };
    let body = format!(
        "<section class=\"section book-detail-page\"><div class=\"container\">\
         <nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\">\
         <a href=\"/\">Главная</a><span aria-hidden=\"true\">/</span>\
         <a href=\"/catalog\">Каталог</a><span aria-hidden=\"true\">/</span>\
         <span aria-current=\"page\">{title}</span></nav>\
         <div class=\"book-detail__layout\">\
         <div class=\"book-detail__art\" aria-hidden=\"true\"><span>П</span></div>\
         <div class=\"book-detail__info\">\
         <p class=\"eyebrow\">Книга · {language}</p><h1>{title}</h1>\
         <p class=\"book-detail__description\">{description}</p>\
         <div class=\"book-detail__actions\">\
         <a class=\"button\" href=\"{action}\" >{label}</a>\
         <span class=\"book-detail__note\">Вход для просмотра каталога не требуется</span>\
         </div></div></div></div></section>{toc}",
        action = if toc.is_some() {
            "#chapters"
        } else {
            "/catalog"
        },
        label = if toc.is_some() {
            "К оглавлению"
        } else {
            "Вернуться к книгам"
        },
        toc = toc.unwrap_or("")
    );
    layout(&book.title, "Карточка книги в каталоге Пандаолейт.", &body)
}

pub fn reader(chapter: &tor_reading::services::PublicChapterPage) -> Html<String> {
    let previous = chapter
        .previous_chapter_id
        .map(|id| format!("<a rel=\"prev\" href=\"/reader/{id}\">Предыдущая глава</a>"))
        .unwrap_or_default();
    let next = chapter
        .next_chapter_id
        .map(|id| format!("<a rel=\"next\" href=\"/reader/{id}\">Следующая глава</a>"))
        .unwrap_or_default();
    let navigation = if previous.is_empty() && next.is_empty() {
        String::new()
    } else {
        format!("<nav class=\"chapter-navigation\" aria-label=\"Переход между главами\">{previous}{next}</nav>")
    };
    let html = include_str!("../templates/reader.html")
        .replace("<!--BOOK_ID-->", &chapter.book_id.to_string())
        .replace("<!--BOOK_TITLE-->", &escape_html(&chapter.book_title))
        .replace("<!--CHAPTER_TITLE-->", &escape_html(&chapter.chapter_title))
        .replace("<!--CHAPTER_VERSION-->", &chapter.version_no.to_string())
        .replace("<!--CHAPTER_NAVIGATION-->", &navigation)
        .replace(
            "<!--CHAPTER_CONTENT-->",
            &tor_reading::domain::renderer::render(&chapter.body_markdown),
        );
    layout(
        &chapter.chapter_title,
        "Опубликованная бесплатная глава.",
        &html,
    )
}

/// Account-only reading: the browser can save a deterministic point in an
/// immutable, currently published, free chapter without requiring JavaScript.
pub fn private_reader(
    chapter_id: u64,
    chapter: &tor_reading::services::PublicChapterPage,
    csrf: &str,
    saved: Option<tor_reading::services::progress::ReadingPlace>,
) -> Html<String> {
    use tor_reading::services::progress::{reader_blocks, valid_text_place};
    let blocks = reader_blocks(&chapter.body_markdown);
    let mut sections = String::new();
    for (index, source) in blocks.iter().enumerate() {
        let block_index = index + 1;
        let markdown = tor_reading::domain::renderer::render(source);
        write!(
            sections,
            r#"<section class="reader-piece" id="p-{block_index}" aria-label="Фрагмент {block_index}">
                 <div class="reader-piece__text reader-markdown">{markdown}</div>
                 <form class="reader-piece__form" method="post" action="/read/{chapter_id}/place">
                    <input type="hidden" name="csrf_token" value="{csrf}">
                    <input type="hidden" name="version_id" value="{version_id}">
                    <input type="hidden" name="block_index" value="{block_index}">
                    <input type="hidden" name="char_offset" value="0">
                    <button type="submit" class="reader-save-button" aria-label="Сохранить после фрагмента {block_index}">Сохранить место здесь</button>
                 </form>
               </section>"#,
            csrf = escape_html(csrf),
            version_id = chapter.version_id
        ).expect("write reader block");
    }
    let valid_saved = saved.filter(|p| valid_text_place(*p, &blocks).is_ok());
    let saved_link = valid_saved
        .map(|p| {
            format!(
                r##"<a class="reader-resume__link" href="#p-{}">Перейти к сохранённому месту</a>"##,
                p.block_index
            )
        })
        .unwrap_or_else(|| "Место ещё не сохранено".to_owned());
    let prev = chapter
        .previous_chapter_id
        .map(|id| format!(r#"<a rel="prev" href="/read/{id}">Предыдущая глава</a>"#))
        .unwrap_or_default();
    let next = chapter
        .next_chapter_id
        .map(|id| format!(r#"<a rel="next" href="/read/{id}">Следующая глава</a>"#))
        .unwrap_or_default();
    let navigation = format!(
        r#"<nav class="chapter-navigation" aria-label="Переход между главами">{prev}{next}</nav>"#
    );
    let body = format!(
        r#"<section class="section reader-page reader-private" aria-labelledby="reader-title">
            <div class="container reader-width">
                <nav class="breadcrumbs" aria-label="Навигационная цепочка">
                    <a href="/me/library">Моя библиотека</a>
                    <span aria-hidden="true">/</span>
                    <a href="/books/{book_id}">{book_title}</a>
                </nav>
                <p class="eyebrow">Читаю с сохранением · Версия {version_no}</p>
                <h1 id="reader-title">{chapter_title}</h1>
                <p class="reader-page__subtitle">После нужного фрагмента нажмите «Сохранить место здесь». 
                  При следующем входе книга появится в библиотеке.</p>
                <div class="reader-resume" role="status">{saved_link}</div>
                {navigation}
                <article class="chapter-content reader-private__content" aria-label="Текст главы">
                    {sections}
                </article>
                {navigation}
                <p class="reader-page__return"><a class="text-link" href="/me/library">К моей библиотеке</a></p>
            </div></section>"#,
        book_id = chapter.book_id,
        book_title = escape_html(&chapter.book_title),
        version_no = chapter.version_no,
        chapter_title = escape_html(&chapter.chapter_title)
    );
    layout(
        &chapter.chapter_title,
        "Чтение с сохранением места в личной библиотеке.",
        &body,
    )
}

pub fn bookmark_prompt(book: &tor_reading::services::PublicBookPage, csrf: &str) -> Html<String> {
    let title = escape_html(&book.title);
    let token = escape_html(csrf);
    let html = format!(
        "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card\"><p class=\"eyebrow\">Личная библиотека</p><h1>Добавить книгу</h1><p>{title}</p><form method=\"post\" action=\"/me/library/{}/save\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><button class=\"button\" type=\"submit\">Сохранить книгу</button></form><p><a href=\"/books/{}\">Вернуться к книге</a></p></div></div></section>",
        book.id, book.id
    );
    layout(
        "Добавить книгу",
        "Сохранить книгу в личной библиотеке.",
        &html,
    )
}

pub fn my_library(
    items: &[tor_reading::services::progress::ContinueReading],
    saved: &tor_reading::services::library::SavedBookPage,
    csrf: &str,
) -> Html<String> {
    let mut cards = String::new();
    if items.is_empty() {
        cards.push_str("<div class=\"library-empty\"><h2>Вы пока не сохраняли место чтения</h2><p>Откройте каталог, чтобы выбрать книгу. Откройте любую бесплатную главу, затем нажмите «Сохранить место здесь». После этого книга появится в вашей истории чтения.</p><a class=\"button\" href=\"/catalog\">Открыть каталог</a></div>");
    } else {
        for item in items {
            let book = escape_html(&item.book_title);
            let chapter = escape_html(&item.chapter_title);
            let bookmark = if item.block_index > 0 {
                format!("#p-{}", item.block_index)
            } else {
                String::new() // published version changed: never resume at an old anchor
            };
            let hint = if item.block_index > 0 {
                format!("Фрагмент {}", item.block_index)
            } else {
                "Глава обновлена — проверьте новое содержание".to_owned()
            };
            write!(cards, "<article class=\"library-card\"><div><p class=\"eyebrow\">Продолжить чтение</p><h2>{book}</h2><p>Глава {}. {chapter}</p><p class=\"library-card__place\">{hint}</p></div><a class=\"button\" href=\"/read/{}{bookmark}\">Продолжить</a></article>", item.chapter_no, item.chapter_id).expect("write HTML");
        }
    }
    let token = escape_html(csrf);
    let mut saved_html = String::new();
    if saved.items.is_empty() {
        saved_html.push_str("<p>Пока нет сохранённых книг.</p>");
    } else {
        for book in &saved.items {
            let title = escape_html(&book.title);
            write!(
                saved_html,
                "<article class=\"library-card\"><div><h3><a href=\"/books/{}\">{title}</a></h3></div><form method=\"post\" action=\"/me/library/{}/remove\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><button class=\"button button--small\" type=\"submit\">Убрать из библиотеки</button></form></article>",
                book.book_id, book.book_id
            )
            .expect("writing saved book");
        }
    }
    if let Some(after) = saved.next_cursor {
        write!(
            saved_html,
            "<p><a href=\"/me/library?after={after}\">Другие сохранённые книги</a></p>"
        )
        .expect("writing library cursor");
    }
    let html = format!(
        "<section class=\"section library-page\"><div class=\"container\"><nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\"><a href=\"/me\">Мой кабинет</a><span aria-hidden=\"true\">/</span><span aria-current=\"page\">Библиотека</span></nav><div class=\"library-heading\"><p class=\"eyebrow\">Моё чтение</p><h1>Продолжить чтение</h1><p>Ваша история чтения доступна только после входа. Платные и закрытые главы сюда не попадают.</p></div><div class=\"library-grid\">{cards}</div><section class=\"section\" aria-label=\"Сохранённые книги\"><h2>Сохранённые книги</h2><div class=\"library-grid\">{saved_html}</div></section></div></section>"
    );
    layout(
        "Моя библиотека",
        "Продолжить чтение в личной библиотеке.",
        &html,
    )
}

pub async fn stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/site.css"),
    )
}

/// NEwBOO public design tokens, components and layout, kept separate from
/// the functioning reader/author styles until their port is independently tested.
pub async fn newboo_stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/newboo-port.css"),
    )
}

pub async fn brand_logo() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_bytes!("../assets/brand/pandaolate-header.png").as_slice(),
    )
}

pub async fn brand_logo_dark() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_bytes!("../assets/brand/pandaolate-header-dark.png").as_slice(),
    )
}

pub fn inactive_account_page(title: &str) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")],
     layout(title, "Аккаунты пока недоступны.", "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card\"><p class=\"eyebrow\">Профиль читателя</p><h1>Аккаунты пока не открыты</h1><p class=\"page-description\">Вход и регистрация будут доступны после завершения проверки безопасности и подключения постоянной базы данных. Сейчас каталог можно просматривать без аккаунта.</p><a class=\"button\" href=\"/catalog\">Перейти в каталог</a></div></div></section>"))
}

#[cfg(test)]
mod tests {
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
            "oldest",
        )
        .0;
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
        assert!(include_bytes!("../assets/brand/pandaolate-header.png").starts_with(b"\x89PNG"));
    }
}
