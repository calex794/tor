//! The original NEwBOO public catalogue layout, sourced from real TOR read data.
use super::{escape_html, layout};
use axum::response::Html;
use std::{collections::HashSet, fmt::Write};
use tor_search::domain::seo_urls::BookUrl;
use tor_search::repositories::catalog::TaxonomyOptions;

mod catalog_active_filters;
mod catalog_pagination;
mod catalog_taxonomy_selects;
mod home_spotlight;

#[derive(Clone, Debug)]
pub struct PublicBookCard {
    pub id: u64,
    pub title: String,
    pub language: String,
    pub author_name: Option<String>,
    pub genres: Vec<String>,
}

/// NEwBOO BookCard layout, with real PostgreSQL-backed author and genre metadata.
fn book_cards(
    books: &[PublicBookCard],
    shelf: bool,
    shelf_label: &str,
    covers: &HashSet<u64>,
) -> String {
    if books.is_empty() {
        return "<div class=\"ui-empty-state\"><h3 class=\"ui-empty-state__title\">Каталог пока наполняется</h3><p class=\"ui-empty-state__text\">В каталоге пока нет опубликованных книг.</p></div>".to_owned();
    }
    let mut html = if shelf {
        format!(
            "<div class=\"home-shelf__track\" aria-label=\"{}\">",
            escape_html(shelf_label)
        )
    } else {
        String::from("<div class=\"catalog-grid\">")
    };
    for book in books {
        let Some(url) = BookUrl::from_id(book.id) else {
            continue;
        };
        let href = url.path();
        let title = escape_html(&book.title);
        let mut details = String::new();
        // Match the original NEwBOO BookCard: public author if present, not
        // invented likes/comments or an extra language/genre metadata block.
        if let Some(author) = book
            .author_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        {
            write!(
                details,
                "<div class=\"book-mini-card__meta\">{}</div>",
                escape_html(author),
            )
            .expect("format String");
        }
        let variant = if shelf {
            "book-mini-card--shelf"
        } else {
            "book-mini-card--compact"
        };
        let cover = if covers.contains(&book.id) {
            format!(
                r#"<img src="/covers/{}/card.webp" alt="" loading="lazy" decoding="async" draggable="false">"#,
                book.id
            )
        } else {
            "<span>Нет обложки</span>".to_owned()
        };
        write!(
            html,
            "<article class=\"book-mini-card {variant}\"><a class=\"book-mini-card__link\" href=\"{href}\" aria-label=\"{title}\"><div class=\"book-mini-card__cover book-shimmer\">{cover}</div><div class=\"book-mini-card__body\"><h3 class=\"book-mini-card__title\">{title}</h3>{details}</div></a></article>",
        )
        .expect("format String");
    }
    html.push_str("</div>");
    html
}

/// Group the already authorized latest-book rows without extra SQL queries.
/// These are shelves of recent books, never a popularity/rating recommendation.
fn recent_genre_shelves(books: &[PublicBookCard], covers: &HashSet<u64>) -> String {
    let mut genres = Vec::<&str>::new();
    for book in books {
        for genre in &book.genres {
            let value = genre.trim();
            if !value.is_empty() && !genres.contains(&value) {
                genres.push(value);
                if genres.len() == 3 {
                    break;
                }
            }
        }
        if genres.len() == 3 {
            break;
        }
    }

    let mut html = String::new();
    for (index, genre) in genres.into_iter().enumerate() {
        let matches: Vec<_> = books
            .iter()
            .filter(|book| book.genres.iter().any(|value| value.trim() == genre))
            .take(4)
            .cloned()
            .collect();
        let genre_html = escape_html(genre);
        let genre_url = encode_search_term(genre);
        let label = format!("Недавно добавленные книги жанра {genre}");
        write!(
            html,
            r#"<section class="section home-genre-section" aria-labelledby="home-genre-{index}">
  <div class="container">
    <div class="home-section-header-row">
      <header class="ui-section-header">
        <h2 class="ui-section-header__title" id="home-genre-{index}">Новинки: {genre_html}</h2>
      </header>
      <a class="ui-button ui-button--ghost ui-button--small" href="/catalog?genre={genre_url}&amp;sort=newest">Все книги жанра</a>
    </div>
    {}
  </div>
</section>"#,
            book_cards(&matches, true, &label, covers)
        )
        .expect("write genre shelf");
    }
    html
}

fn home_body(books: &[PublicBookCard], latest: bool, covers: &HashSet<u64>) -> Html<String> {
    let fallback = r#"<div class="home-showcase">
        <div><p class="ui-page-header__kicker">Библиотека платформы</p>
        <p class="home-showcase__text">Читайте опубликованные книги. Выберите доступную главу и продолжайте чтение в личной библиотеке.</p></div>
        <a class="ui-button ui-button--ghost" href="/catalog">Открыть каталог</a>
    </div>"#;
    let hero = if latest && !books.is_empty() {
        home_spotlight::render(books, covers)
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
        .replace(
            "<!--PUBLIC_BOOKS-->",
            &book_cards(books, latest, "Недавно добавленные книги", covers),
        )
        .replace(
            "<!--GENRE_SHELVES-->",
            &if latest {
                recent_genre_shelves(books, covers)
            } else {
                String::new()
            },
        );
    layout(
        "Главная",
        "Пандаолейт — книги и переводы для чтения онлайн.",
        &content,
    )
}

/// Demo mode cannot claim recent insertion order; only the PG projection can.
pub fn home(books: &[PublicBookCard]) -> Html<String> {
    home_body(books, false, &HashSet::new())
}

/// F-04 read model supplies descending creation identity, never a false daily top
/// or a claim about publication time, rating or personal recommendations.
pub fn home_recent_with_covers(books: &[PublicBookCard], covers: &HashSet<u64>) -> Html<String> {
    home_body(books, true, covers)
}

pub fn catalog(books: &[PublicBookCard]) -> Html<String> {
    let content = include_str!("../../templates/catalog.html")
        .replace(
            "<!--PUBLIC_BOOKS-->",
            &book_cards(books, false, "", &HashSet::new()),
        )
        .replace("<!--SEARCH_FORM-->", "")
        .replace("<!--ACTIVE_FILTERS-->", "");
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

/// Presentation-only query state; rendering never makes independent PG reads.
pub struct CatalogPageQuery<'a> {
    pub query: &'a str,
    pub language: Option<&'a str>,
    pub genre: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub fandom: Option<&'a str>,
    pub cursor: Option<&'a str>,
    pub previous_cursor: Option<&'a str>,
    pub page_number: u32,
    pub sort: &'a str,
    pub total: Option<u64>,
    pub options: Option<&'a TaxonomyOptions>,
}

pub fn catalog_search_with_covers(
    books: &[PublicBookCard],
    request: CatalogPageQuery<'_>,
    covers: &HashSet<u64>,
) -> Html<String> {
    let active_filters = catalog_active_filters::render(&request);
    let pagination = catalog_pagination::render(&request);
    let CatalogPageQuery {
        query,
        language,
        genre,
        tag,
        fandom,
        cursor: _,
        previous_cursor: _,
        page_number: _,
        sort,
        total,
        options,
    } = request;
    let oldest = if sort == "oldest" { " selected" } else { "" };
    let newest = if sort == "newest" { " selected" } else { "" };
    let title = if sort == "title" { " selected" } else { "" };
    let author = if sort == "author" { " selected" } else { "" };
    let mut controls = format!(
        r#"<form class="catalog-filters" action="/catalog" method="get" role="search" aria-label="Фильтры каталога"><input class="ui-input catalog-filters__search" id="book-query" type="search" name="q" maxlength="80" value="{}" placeholder="Название или автор" aria-label="Название или автор"><input class="ui-input" id="book-genre" type="text" name="genre" list="catalog-genres" maxlength="40" value="{}" placeholder="Жанр (фэнтези)" aria-label="Жанр"><input class="ui-input" id="book-tag" type="text" name="tag" list="catalog-tags" maxlength="40" value="{}" placeholder="Тег" aria-label="Тег"><input class="ui-input" id="book-fandom" type="text" name="fandom" list="catalog-fandoms" maxlength="40" value="{}" placeholder="Фэндом" aria-label="Фэндом"><input class="ui-input catalog-filters__select" id="book-language" name="language" maxlength="32" placeholder="Язык перевода (ru)" value="{}" aria-label="Язык перевода"><label class="catalog-filters__sort-label" for="book-sort">Порядок</label><select class="ui-input catalog-filters__select" id="book-sort" name="sort" aria-label="Сортировка"><option value="oldest"{}>Сначала добавленные</option><option value="newest"{}>Сначала новые</option><option value="title"{}>По названию</option><option value="author"{}>По автору</option></select><button class="ui-button ui-button--primary catalog-filters__submit" type="submit">Найти</button></form>"#,
        escape_html(query),
        escape_html(genre.unwrap_or("")),
        escape_html(tag.unwrap_or("")),
        escape_html(fandom.unwrap_or("")),
        escape_html(language.unwrap_or("")),
        oldest,
        newest,
        title,
        author,
    );
    if !query.trim().is_empty()
        || language.is_some()
        || genre.is_some()
        || tag.is_some()
        || fandom.is_some()
    {
        controls = controls.replacen(
            "</form>",
            "<a class=\"ui-button ui-button--ghost\" href=\"/catalog\">Сбросить</a></form>",
            1,
        );
    }
    controls = catalog_taxonomy_selects::apply(controls, options, genre, tag, fandom);
    controls.push_str(&pagination);
    let count_markup = total
        .map(|number| {
            format!("<p class=\"catalog-result-count\" role=\"status\">Найдено книг: {number}</p>")
        })
        .unwrap_or_default();
    let content = include_str!("../../templates/catalog.html")
        .replace(
            "<!--PUBLIC_BOOKS-->",
            &if books.is_empty()
                && (!query.trim().is_empty()
                    || language.is_some()
                    || genre.is_some()
                    || tag.is_some()
                    || fandom.is_some())
            {
                "<div class=\"ui-empty-state\"><h3>Книги не найдены</h3><p>Попробуйте изменить условия поиска.</p></div>".to_owned()
            } else {
                book_cards(books, false, "", covers)
            },
        )
        .replace("<!--SEARCH_FORM-->", &controls)
        .replace("<!--ACTIVE_FILTERS-->", &format!("{active_filters}{count_markup}"));
    layout(
        "Каталог",
        "Поиск опубликованных книг на Пандаолейт.",
        &content,
    )
}

#[cfg(test)]
pub fn home_recent(books: &[PublicBookCard]) -> Html<String> {
    home_body(books, true, &HashSet::new())
}

#[cfg(test)]
pub fn catalog_search(books: &[PublicBookCard], request: CatalogPageQuery<'_>) -> Html<String> {
    catalog_search_with_covers(books, request, &HashSet::new())
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
    fn f_04_recent_genres_are_from_real_rows_without_untrusted_html_or_fake_popularity() {
        let mut latest = book(12, "Новейшая");
        latest.genres = vec!["Научная фантастика".into(), "Фэнтези".into()];
        let mut older = book(10, "Другая");
        older.genres = vec!["Фэнтези".into(), "<img onerror=x>".into()];
        let mut last = book(9, "Третья");
        last.genres = vec!["Детектив".into()];
        let html = home_recent(&[latest, older, last]).0;
        assert!(html.contains("Новинки: Научная фантастика"));
        assert!(html.contains("Новинки: Фэнтези"));
        assert!(
            !html.contains("Новинки: Детектив"),
            "show at most three genres"
        );
        assert!(html.contains("/catalog?genre=%D0%9D%D0%B0%D1%83%D1%87%D0%BD%D0%B0%D1%8F%20%D1%84%D0%B0%D0%BD%D1%82%D0%B0%D1%81%D1%82%D0%B8%D0%BA%D0%B0&amp;sort=newest"));
        assert!(html.contains("&lt;img onerror=x&gt;"));
        assert!(!html.contains("<img onerror=x>"));
        assert!(!html.contains("Популярные книги"));
        assert!(!html.contains("Рекомендуем для вас"));

        let demo = home(&[book(1, "Не новая")]).0;
        assert!(!demo.contains("home-genre-section"));
        assert!(!home_recent(&[]).0.contains("home-genre-section"));
    }

    #[test]
    fn f_04_genre_shelves_respect_four_card_limit_and_latest_order() {
        let books = (1..=7)
            .rev()
            .map(|id| {
                let mut book = book(id, &format!("Книга {id}"));
                book.genres = vec!["фэнтези".into()];
                book
            })
            .collect::<Vec<_>>();
        let shelves = recent_genre_shelves(&books, &HashSet::new());
        assert_eq!(
            shelves
                .matches("class=\"book-mini-card book-mini-card--shelf\"")
                .count(),
            4
        );
        assert!(shelves.contains("href=\"/books/7\""));
        assert!(!shelves.contains("href=\"/books/3\""));
    }

    #[test]
    fn empty_recent_home_never_invents_featured_books() {
        let html = home_recent(&[]).0;
        assert!(!html.contains("home-spotlight-card"));
        assert!(html.contains("Каталог пока наполняется"));
    }

    #[test]
    fn only_verified_public_cover_ids_render_same_origin_images() {
        let cards = [book(8, "Книга 8"), book(3, "Книга 3")];
        let available = HashSet::from([8_u64]);
        let home = home_recent_with_covers(&cards, &available).0;
        assert!(home.contains(r#"src="/covers/8/main.webp""#));
        assert!(home.contains(r#"src="/covers/8/card.webp""#));
        assert!(!home.contains("/covers/3/"));
        assert!(home.contains("Нет обложки"));
        assert!(!home.contains("staging/covers"));
        let catalog = catalog_search_with_covers(
            &cards,
            CatalogPageQuery {
                query: "",
                language: None,
                genre: None,
                tag: None,
                fandom: None,
                cursor: None,
                previous_cursor: None,
                page_number: 1,
                sort: "oldest",
                total: Some(2),
                options: None,
            },
            &available,
        )
        .0;
        assert!(catalog.contains(r#"src="/covers/8/card.webp""#));
        assert!(!catalog.contains("/covers/3/"));
    }

    #[test]
    fn h_10_cards_match_original_public_neboo_author_only_metadata() {
        let mut card = book(42, "Пробная новелла");
        card.author_name = Some("Имя автора".into());
        card.genres = vec!["Детектив".into()];
        let html = catalog(&[card]).0;
        assert!(html.contains("book-mini-card__cover book-shimmer"));
        assert!(html.contains("<div class=\"book-mini-card__meta\">Имя автора</div>"));
        assert!(!html.contains("Язык: ru"));
        assert!(!html.contains("Жанры:"));
        assert!(!html.contains("Лайков:"));
    }

    #[test]
    fn h_11_card_author_is_html_escaped() {
        let mut card = book(43, "Название");
        card.author_name = Some("<img src=x onerror=evil()>".into());
        let html = catalog(&[card]).0;
        assert!(html.contains("&lt;img src=x onerror=evil()&gt;"));
        assert!(!html.contains("<img src=x"));
    }

    #[test]
    fn demo_home_must_not_claim_latest_order() {
        let html = home(&[book(1, "Demo")]).0;
        assert!(!html.contains("Недавно добавленные"));
        assert!(!html.contains("home-spotlight-card"));
        assert!(html.contains("Опубликованные книги"));
    }
}
