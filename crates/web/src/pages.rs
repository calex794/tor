//! Server-rendered demo pages based on BookaRust source d11b27b43125b93e59809147e8e687203fa0753b.
use axum::{
    http::header,
    response::{Html, IntoResponse},
};
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct PublicBookCard {
    pub id: u64,
    pub title: String,
    pub language: String,
}

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

fn book_cards(books: &[PublicBookCard]) -> String {
    if books.is_empty() {
        return "<p class=\"empty-state\">В каталоге пока нет опубликованных книг.</p>".to_owned();
    }
    let mut html = String::new();
    for book in books {
        let title = escape_html(&book.title);
        let language = escape_html(&book.language);
        write!(html,
            "<article class=\"book-card\"><div class=\"book-card__cover\" aria-hidden=\"true\">К</div><div class=\"book-card__body\"><span class=\"book-card__label\">Книга · {language}</span><h3 class=\"book-card__title\"><a href=\"/books/{}\">{title}</a></h3></div></article>",
            book.id).expect("format String");
    }
    html
}

pub fn home(books: &[PublicBookCard]) -> Html<String> {
    let content =
        include_str!("../templates/home.html").replace("<!--PUBLIC_BOOKS-->", &book_cards(books));
    layout(
        "Главная",
        "Пандаолейт — демонстрационный каталог без пользовательских данных.",
        &content,
    )
}

pub fn catalog(books: &[PublicBookCard]) -> Html<String> {
    let content = include_str!("../templates/catalog.html")
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books))
        .replace("<!--SEARCH_FORM-->", "");
    layout(
        "Каталог",
        "Опубликованные книги и переводы на Пандаолейт.",
        &content,
    )
}

fn encode_search_term(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            result.push(char::from(byte));
        } else {
            write!(result, "%{byte:02X}").expect("string write");
        }
    }
    result
}

pub fn catalog_search(books: &[PublicBookCard], query: &str, cursor: Option<&str>) -> Html<String> {
    let mut controls = format!(
        r#"<form action="/catalog" method="get" role="search" class="catalog-search"><label for="book-query">Название книги</label><input id="book-query" type="search" name="q" maxlength="80" value="{}" required><button class="button button--small" type="submit">Найти</button></form>"#,
        escape_html(query),
    );
    if let Some(cursor) = cursor {
        write!(
            controls,
            r#"<nav aria-label="Следующая страница поиска"><a href="/catalog?q={}&amp;after={}">Следующие книги</a></nav>"#,
            encode_search_term(query),
            escape_html(cursor),
        ).expect("string write");
    }
    let content = include_str!("../templates/catalog.html")
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books))
        .replace("<!--SEARCH_FORM-->", &controls);
    layout(
        "Каталог",
        "Поиск опубликованных книг на Пандаолейт.",
        &content,
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

pub fn registration_form(csrf: &str) -> Html<String> {
    let content = include_str!("../templates/register_active.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf));
    layout("Регистрация", "Создать личный кабинет читателя.", &content)
}

pub fn login_form(csrf: &str) -> Html<String> {
    let content = include_str!("../templates/login_active.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf));
    layout("Вход", "Войти в личный кабинет.", &content)
}

pub fn my_account(name: &str, csrf: &str, behavior_opt_in: bool, configured: bool) -> Html<String> {
    let state = if behavior_opt_in {
        "Анализ переходов включён по вашему согласию."
    } else {
        "Анализ ваших переходов отключён."
    };
    let controls = if configured || behavior_opt_in {
        let (choice, button) = if behavior_opt_in {
            ("disable", "Отключить и удалить мою последовательность")
        } else {
            ("enable", "Включить анализ моих переходов")
        };
        format!(
            "<section aria-label=\"Настройки приватности\"><h2>Приватность</h2><p>{state}</p><p>Технические общие счётчики не содержат личной истории. Персональные последовательности хранятся только в памяти этого сервера до 30 минут бездействия.</p><form method=\"post\" action=\"/me/behavior\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"choice\" value=\"{choice}\"><button class=\"button button--quiet\" type=\"submit\">{button}</button></form></section>",
            escape_html(csrf),
        )
    } else {
        "<section aria-label=\"Настройки приватности\"><h2>Приватность</h2><p>Анализ ваших переходов отключён. Сервер не настроен для персонального анализа.</p></section>".to_owned()
    };
    let content = include_str!("../templates/me.html")
        .replace("<!--DISPLAY_NAME-->", &escape_html(name))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--RIL_BEHAVIOR_CONTROLS-->", &controls);
    // Only the authenticated private page changes its navigation.
    let html = layout("Мой кабинет", "Учётная запись и личный кабинет.", &content);
    Html(
        html.0
            .replace(
                "<a class=\"site-nav__link\" href=\"/login\">Войти</a>",
                "<a class=\"site-nav__link\" href=\"/me\" aria-current=\"page\">Мой кабинет</a>",
            )
            .replace(
                "<a class=\"button button--small\" href=\"/register\">Регистрация</a>",
                "",
            ),
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
        }]);
        assert!(page.0.contains("&lt;script&gt;"));
        assert!(!page.0.contains("<script>"));
        assert!(page.0.contains("href=\"/books/9\""));
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
}
