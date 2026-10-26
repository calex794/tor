//! The original NEwBOO public catalogue layout, sourced from real TOR read data.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct PublicBookCard {
    pub id: u64,
    pub title: String,
    pub language: String,
    pub author_name: Option<String>,
    pub genres: Vec<String>,
}

/// NEwBOO BookCard layout, with real PostgreSQL-backed author and genre metadata.
fn book_cards(books: &[PublicBookCard], shelf: bool) -> String {
    if books.is_empty() {
        return "<div class=\"ui-empty-state\"><h3 class=\"ui-empty-state__title\">Каталог пока наполняется</h3><p class=\"ui-empty-state__text\">В каталоге пока нет опубликованных книг.</p></div>".to_owned();
    }
    let mut html = if shelf {
        String::from("<div class=\"home-shelf__track\" aria-label=\"Недавно добавленные книги\">")
    } else {
        String::from("<div class=\"catalog-grid\">")
    };
    for book in books {
        let title = escape_html(&book.title);
        let language = escape_html(&book.language);
        let mut details = String::new();
        if let Some(author) = &book.author_name {
            write!(
                details,
                "<p class=\"book-card__meta book-mini-card__meta\">Автор: {}</p>",
                escape_html(author),
            )
            .expect("format String");
        }
        if !book.genres.is_empty() {
            write!(
                details,
                "<p class=\"book-card__meta book-mini-card__meta\">Жанры: {}</p>",
                escape_html(&book.genres.join(", ")),
            )
            .expect("format String");
        }
        let variant = if shelf {
            "book-mini-card--shelf"
        } else {
            "book-mini-card--compact"
        };
        write!(
            html,
            "<article class=\"book-mini-card {variant}\"><a class=\"book-mini-card__link\" href=\"/books/{}\" aria-label=\"{title}\"><div class=\"book-mini-card__cover\"><span>Нет обложки</span></div><div class=\"book-mini-card__body\"><h3 class=\"book-mini-card__title\">{title}</h3><p class=\"book-mini-card__meta\">Язык: {language}</p>{details}</div></a></article>",
            book.id,
        )
        .expect("format String");
    }
    html.push_str("</div>");
    html
}

/// The public NEwBOO spotlight is populated only from actual published rows.
fn spotlight(book: &PublicBookCard) -> String {
    let title = escape_html(&book.title);
    let details = book
        .author_name
        .as_deref()
        .map(escape_html)
        .unwrap_or_else(|| format!("Язык: {}", escape_html(&book.language)));
    format!(
        r#"<a class="home-spotlight-card" href="/books/{id}" aria-label="Открыть книгу {title}">
           <div class="home-spotlight-card__inner">
             <div class="home-spotlight-card__content">
               <span class="home-spotlight-card__eyebrow">Недавно добавлена</span>
               <h2 class="home-spotlight-card__title">{title}</h2>
               <p class="home-spotlight-card__text">{details}</p>
             </div>
             <div class="home-spotlight-card__cover-wrap">
               <div class="home-spotlight-card__cover home-spotlight-card__cover--empty">Нет обложки</div>
             </div>
           </div>
         </a>"#,
        id = book.id
    )
}

fn home_body(books: &[PublicBookCard], latest: bool) -> Html<String> {
    let fallback = r#"<div class="home-showcase">
        <div><p class="ui-page-header__kicker">Библиотека платформы</p>
        <p class="home-showcase__text">Читайте опубликованные книги. Выберите доступную главу и продолжайте чтение в личной библиотеке.</p></div>
        <a class="ui-button ui-button--ghost" href="/catalog">Открыть каталог</a>
    </div>"#;
    let hero = if latest {
        books
            .first()
            .map(spotlight)
            .unwrap_or_else(|| fallback.to_owned())
    } else {
        fallback.to_owned()
    };
    let content = include_str!("../../templates/home.html")
        .replace("<!--HOME_SPOTLIGHT-->", &hero)
        .replace(
            "<!--BOOKS_HEADING-->",
            if latest {
                "Недавно добавленные"
            } else {
                "Опубликованные книги"
            },
        )
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books, latest));
    layout(
        "Главная",
        "Пандаолейт — книги и переводы для чтения онлайн.",
        &content,
    )
}

/// Demo mode cannot claim recent insertion order; only the PG projection can.
pub fn home(books: &[PublicBookCard]) -> Html<String> {
    home_body(books, false)
}

/// F-04 read model supplies descending creation identity, never a false daily top
/// or a claim about publication time, rating or personal recommendations.
pub fn home_recent(books: &[PublicBookCard]) -> Html<String> {
    home_body(books, true)
}

pub fn catalog(books: &[PublicBookCard]) -> Html<String> {
    let content = include_str!("../../templates/catalog.html")
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books, false))
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

pub fn catalog_search(
    books: &[PublicBookCard],
    query: &str,
    language: Option<&str>,
    genre: Option<&str>,
    cursor: Option<&str>,
    sort: &str,
    total: Option<u64>,
) -> Html<String> {
    let oldest = if sort == "oldest" { " selected" } else { "" };
    let newest = if sort == "newest" { " selected" } else { "" };
    let mut controls = format!(
        r#"<form class="catalog-filters" action="/catalog" method="get" role="search" aria-label="Фильтры каталога"><input class="ui-input catalog-filters__search" id="book-query" type="search" name="q" maxlength="80" value="{}" placeholder="Название или автор" aria-label="Название или автор"><input class="ui-input" id="book-genre" type="text" name="genre" maxlength="40" value="{}" placeholder="Жанр (фэнтези)" aria-label="Жанр"><input class="ui-input catalog-filters__select" id="book-language" name="language" maxlength="32" placeholder="Язык перевода (ru)" value="{}" aria-label="Язык перевода"><label class="catalog-filters__sort-label" for="book-sort">Порядок</label><select class="ui-input catalog-filters__select" id="book-sort" name="sort" aria-label="Сортировка"><option value="oldest"{}>Сначала добавленные</option><option value="newest"{}>Сначала новые</option></select><button class="ui-button ui-button--primary catalog-filters__submit" type="submit">Найти</button></form>"#,
        escape_html(query),
        escape_html(genre.unwrap_or("")),
        escape_html(language.unwrap_or("")),
        oldest,
        newest,
    );
    if let Some(cursor) = cursor {
        let language_param = language
            .filter(|value| !value.is_empty())
            .map(|value| format!("&amp;language={}", encode_search_term(value)))
            .unwrap_or_default();
        let genre_param = genre
            .filter(|value| !value.is_empty())
            .map(|value| format!("&amp;genre={}", encode_search_term(value)))
            .unwrap_or_default();
        write!(
            controls,
            r#"<nav class="catalog-pagination" aria-label="Следующая страница поиска"><a class="ui-button ui-button--ghost" href="/catalog?q={}{}{}&amp;sort={}&amp;after={}">Показать ещё</a></nav>"#,
            encode_search_term(query),
            language_param,
            genre_param,
            escape_html(sort),
            escape_html(cursor),
        ).expect("string write");
    }
    let count_markup = total
        .map(|number| {
            format!("<p class=\"catalog-result-count\" role=\"status\">Найдено книг: {number}</p>")
        })
        .unwrap_or_default();
    let content = include_str!("../../templates/catalog.html")
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books, false))
        .replace("<!--SEARCH_FORM-->", &format!("{controls}{count_markup}"));
    layout(
        "Каталог",
        "Поиск опубликованных книг на Пандаолейт.",
        &content,
    )
}

#[cfg(test)]
mod home_tests {
    use super::*;

    fn book(id: u64, title: &str) -> PublicBookCard {
        PublicBookCard {
            id,
            title: title.to_owned(),
            language: "ru".to_owned(),
            author_name: None,
            genres: Vec::new(),
        }
    }

    #[test]
    fn recent_home_has_real_spotlight_and_shelf_without_fake_ranking() {
        let html = home_recent(&[book(8, "Последняя книга"), book(3, "Ранняя")]).0;
        assert!(html.contains("home-spotlight-card"));
        assert!(html.contains("href=\"/books/8\""));
        assert!(html.contains("Недавно добавленные"));
        assert!(html.contains("Ранняя"));
        assert!(!html.contains("Топ дня"));
        assert!(!html.contains("Рекомендуем для вас"));
    }

    #[test]
    fn recent_home_escapes_untrusted_metadata_everywhere() {
        let html = home_recent(&[PublicBookCard {
            id: 17,
            title: "<script>x</script>".into(),
            language: "ru".into(),
            author_name: Some("\" onclick=\"evil".into()),
            genres: vec!["<img>".into()],
        }])
        .0;
        assert!(!html.contains("<script>"));
        assert!(!html.contains("onclick=\"evil"));
        assert!(!html.contains("<img>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&quot; onclick=&quot;evil"));
    }

    #[test]
    fn empty_recent_home_never_invents_featured_books() {
        let html = home_recent(&[]).0;
        assert!(!html.contains("home-spotlight-card"));
        assert!(html.contains("Каталог пока наполняется"));
    }

    #[test]
    fn demo_home_must_not_claim_latest_order() {
        let html = home(&[book(1, "Demo")]).0;
        assert!(!html.contains("Недавно добавленные"));
        assert!(!html.contains("home-spotlight-card"));
        assert!(html.contains("Опубликованные книги"));
    }
}
