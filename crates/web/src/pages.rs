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
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books));
    layout(
        "Каталог",
        "Опубликованные книги и переводы на Пандаолейт.",
        &content,
    )
}

/// The S0 demo and the PostgreSQL book view share exactly one page shell.
pub fn book_detail(book: &PublicBookCard) -> Html<String> {
    book_detail_body(book, None)
}

/// Public TOC: paid metadata can be listed but paid bodies are never fetched.
pub fn book_detail_with_chapters(book: &tor_reading::services::PublicBookPage) -> Html<String> {
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
            &crate::safe_markdown::render(&chapter.body_markdown),
        );
    layout(
        &chapter.chapter_title,
        "Опубликованная бесплатная глава.",
        &html,
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

pub fn my_account(name: &str, csrf: &str) -> Html<String> {
    let content = include_str!("../templates/me.html")
        .replace("<!--DISPLAY_NAME-->", &escape_html(name))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf));
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

pub fn my_library(items: &[tor_reading::services::progress::ContinueReading]) -> Html<String> {
    let mut cards = String::new();
    if items.is_empty() {
        cards.push_str("<div class=\"library-empty\"><h2>Вы пока не сохраняли место чтения</h2><p>Откройте каталог, чтобы выбрать книгу. Сохранение положения в главе появится в следующем обновлении.</p><a class=\"button\" href=\"/catalog\">Открыть каталог</a></div>");
    } else {
        for item in items {
            let book = escape_html(&item.book_title);
            let chapter = escape_html(&item.chapter_title);
            write!(cards, "<article class=\"library-card\"><div><p class=\"eyebrow\">Продолжить чтение</p><h2>{book}</h2><p>Глава {}. {chapter}</p></div><a class=\"button\" href=\"/reader/{}\">Читать дальше</a></article>", item.chapter_no, item.chapter_id).expect("write HTML");
        }
    }
    let html = format!(
        "<section class=\"section library-page\"><div class=\"container\"><nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\"><a href=\"/me\">Мой кабинет</a><span aria-hidden=\"true\">/</span><span aria-current=\"page\">Библиотека</span></nav><div class=\"library-heading\"><p class=\"eyebrow\">Моё чтение</p><h1>Продолжить чтение</h1><p>Ваша история чтения доступна только после входа. Платные и закрытые главы сюда не попадают.</p></div><div class=\"library-grid\">{cards}</div></div></section>"
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
    fn empty_catalog_is_explicit() {
        assert!(catalog(&[])
            .0
            .contains("В каталоге пока нет опубликованных книг"));
    }
}
