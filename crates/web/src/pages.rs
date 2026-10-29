//! Server-rendered demo pages based on BookaRust source d11b27b43125b93e59809147e8e687203fa0753b.
use axum::{
    http::header,
    response::{Html, IntoResponse},
};
use std::fmt::Write;

mod account;
mod layer_storefront;
mod library;
pub use account::{
    account_error_page, login_form, my_account, registration_form, registration_form_with_error,
};
pub use layer_storefront::{
    offer_page as layer_offer_page, purchase_error as layer_purchase_error,
    purchased_page as purchased_layers_page,
};
pub use library::{my_library, my_library_with_options, my_library_with_purchases};
mod reader_layer;
pub use reader_layer::layer_reader_shell;
mod reader_private;
#[allow(unused_imports)]
pub use reader_private::private_reader;
pub use reader_private::private_reader_with_demand;
mod paywall;
pub use paywall::paid_chapter_paywall;
mod reader_paid;
pub use reader_paid::{paid_reader_shell, purchased_chapters_page};
mod public_catalog;
mod seo_head;
pub use public_catalog::{
    catalog, catalog_search_with_covers, home, home_recent_with_covers, CatalogPageQuery,
    PublicBookCard,
};

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
    layout_with_seo(title, description, content, seo_head::default_head())
}

fn layout_with_seo(
    title: &str,
    description: &str,
    content: &str,
    seo_markup: &str,
) -> Html<String> {
    Html(
        include_str!("../templates/layout.html")
            .replace("<!--PAGE_TITLE-->", &escape_html(title))
            .replace("<!--PAGE_DESCRIPTION-->", &escape_html(description))
            .replace("<!--PAGE_SEO_HEAD-->", seo_markup)
            .replace("<!--PAGE_CONTENT-->", content),
    )
}

/// The S0 demo and the PostgreSQL book view share exactly one page shell.
pub fn book_detail(book: &PublicBookCard) -> Html<String> {
    book_detail_body(book, None, None, false, None, false)
}

/// Public TOC: paid metadata can be listed but paid bodies are never fetched.
pub fn book_detail_with_chapters_cover(
    book: &tor_reading::services::PublicBookPage,
    limit: u16,
    has_cover: bool,
) -> Html<String> {
    book_detail_with_chapters_cover_internal(book, limit, has_cover, false)
}

fn book_detail_with_chapters_cover_internal(
    book: &tor_reading::services::PublicBookPage,
    limit: u16,
    has_cover: bool,
    batch_enabled: bool,
) -> Html<String> {
    let card = PublicBookCard {
        id: book.id,
        title: book.title.clone(),
        language: book.language.clone(),
        author_name: (!book.author_name.trim().is_empty()).then(|| book.author_name.clone()),
        genres: book.genres.clone(),
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
                    "<li class=\"chapter-toc__item chapter-toc__item--paid\"><a href=\"/reader/{}\">Глава {}. {title}</a><span class=\"chapter-toc__badge\">Платная · нужен доступ</span></li>",
                    chapter.id, chapter.number
                )
                .expect("writing String");
            } else {
                write!(
                    items,
                    "<li class=\"chapter-toc__item\"><a href=\"/reader/{}\">Глава {}. {title}</a><span class=\"chapter-toc__badge chapter-toc__badge--free\">Бесплатно</span></li>",
                    chapter.id, chapter.number
                )
                .expect("writing String");
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
    let section = format!(
        "<section class=\"book-section-card chapter-toc\" id=\"chapters\" aria-labelledby=\"toc-title\"><p class=\"book-section-card__kicker\">Оглавление</p><h2 class=\"book-section-card__title\" id=\"toc-title\">Главы книги</h2>{items}</section>"
    );
    // The CTA is only a chapter already verified as public and free by PgReader.
    // A paid TOC link only opens a public metadata paywall, never a grant.
    let first_free = book
        .chapters
        .iter()
        .find(|chapter| !chapter.is_paid)
        .map(|chapter| chapter.id);
    book_detail_body(
        &card,
        Some(&section),
        first_free,
        has_cover,
        Some(&book.description),
        batch_enabled
            && book
                .chapters
                .iter()
                .filter(|chapter| chapter.is_paid)
                .take(2)
                .count()
                == 2,
    )
}

/// F-05: two independent public indicators, copied from NEwBOO's
/// translation/work split. No personal pairwise rank appears in this card.
pub fn book_detail_with_chapters_cover_stars(
    book: &tor_reading::services::PublicBookPage,
    limit: u16,
    has_cover: bool,
    stars: &tor_recommendations::repositories::BookStars,
) -> Html<String> {
    book_detail_with_chapters_cover_stars_batch(book, limit, has_cover, stars, false)
}

pub fn book_detail_with_chapters_cover_stars_batch(
    book: &tor_reading::services::PublicBookPage,
    limit: u16,
    has_cover: bool,
    stars: &tor_recommendations::repositories::BookStars,
    batch_enabled: bool,
) -> Html<String> {
    let html = book_detail_with_chapters_cover_internal(book, limit, has_cover, batch_enabled);
    Html(
        html.0
            .replace("<!--BOOK_RATINGS-->", &book_star_strip(book.id, stars)),
    )
}

fn book_star_strip(book_id: u64, stars: &tor_recommendations::repositories::BookStars) -> String {
    use tor_recommendations::repositories::StarTotal;
    fn value(score: StarTotal, kind: &str, label: &str) -> String {
        let (number, details) = if let Some(average) = score.display_average() {
            (average, format!("Голосов: {}", score.votes))
        } else {
            ("—".to_owned(), "Пока нет оценок".to_owned())
        };
        format!(
            "<div class=\"book-rating-summary__item book-rating-summary__item--{kind}\">
             <span class=\"book-rating-summary__glyph\" aria-hidden=\"true\">☆</span>
             <span class=\"book-rating-summary__name\">{label}</span>
             <strong class=\"book-rating-summary__value\" aria-label=\"{label}: {number} из 5\">{number}</strong>
             <span class=\"book-rating-summary__votes\">{details}</span></div>"
        )
    }
    format!(
        "<section class=\"book-rating-summary\" aria-label=\"Оценки книги\">
         {work}{translation}
         <a class=\"book-rating-summary__action\" href=\"/me/books/{book_id}/rate\">
          Оценить произведение и перевод</a></section>",
        work = value(stars.work, "work", "Произведение"),
        translation = value(stars.translation, "translation", "Перевод"),
    )
}

fn book_detail_body(
    book: &PublicBookCard,
    toc: Option<&str>,
    first_free: Option<u64>,
    has_cover: bool,
    published_description: Option<&str>,
    batch_enabled: bool,
) -> Html<String> {
    let title = escape_html(&book.title);
    let language = escape_html(&book.language);
    let (description_html, meta_description) = match published_description {
        Some(raw) if !raw.trim().is_empty() => {
            let clean = raw.trim();
            let short = clean.split_whitespace().collect::<Vec<_>>().join(" ");
            (
                escape_html(clean).replace('\n', "<br>"),
                short.chars().take(180).collect::<String>(),
            )
        }
        Some(_) => (
            "Аннотация пока не добавлена.".to_owned(),
            format!("«{}» — опубликованная книга на Пандаолейт.", book.title),
        ),
        None => {
            let text = if toc.is_some() {
                "Откройте оглавление, чтобы читать доступные главы бесплатно. Платные главы не открываются без проверки прав."
            } else {
                "Карточка опубликованной книги. Оглавление и чтение глав появятся после подключения читалки."
            };
            (
                text.to_owned(),
                "Карточка книги в каталоге Пандаолейт.".to_owned(),
            )
        }
    };
    let action = if let Some(id) = first_free {
        format!("<a class=\"ui-button ui-button--primary\" href=\"/reader/{id}\">Читать доступную главу</a>")
    } else if toc.is_some() {
        "<a class=\"ui-button ui-button--primary\" href=\"#chapters\">Посмотреть главы</a>"
            .to_owned()
    } else {
        "<a class=\"ui-button ui-button--ghost\" href=\"/catalog\">Вернуться к книгам</a>"
            .to_owned()
    };
    let library_action = if toc.is_some() {
        format!(
            "<a class=\"ui-button ui-button--ghost book-hero-actions__library\" href=\"/me/library/add/{}\">Добавить в библиотеку</a>",
            book.id
        )
    } else {
        String::new()
    };
    let mut credits = String::new();
    if let Some(author) = &book.author_name {
        write!(
            credits,
            "<span class=\"book-hero-card__credit\"><span class=\"book-hero-card__credit-label\">Автор</span>{}</span>",
            escape_html(author)
        )
        .expect("write author");
    }
    if !book.genres.is_empty() {
        write!(
            credits,
            "<span class=\"book-hero-card__credit\"><span class=\"book-hero-card__credit-label\">Жанры</span>{}</span>",
            escape_html(&book.genres.join(", "))
        )
        .expect("write genres");
    }
    let credits = if credits.is_empty() {
        String::new()
    } else {
        format!("<div class=\"book-hero-card__credits\">{credits}</div>")
    };
    let cover = if has_cover {
        format!(
            r#"<img class="book-cover-xl" src="/covers/{}/main.webp" alt="Обложка книги «{title}»" decoding="async" draggable="false">"#,
            book.id
        )
    } else {
        r#"<div class="book-cover-xl" aria-label="Обложка пока не загружена"><span>Нет обложки</span></div>"#
            .to_owned()
    };
    let discussion = if toc.is_some() {
        format!(
            "<section class=\"book-section-card\" aria-label=\"Комментарии\"><h2>Обсуждение книги</h2><a class=\"text-link\" href=\"/books/{}/comments\">Читать и оставить комментарий</a></section>",
            book.id
        )
    } else {
        String::new()
    };
    let content = include_str!("../templates/book_detail.html")
        .replace("<!--BOOK_COVER-->", &cover)
        .replace("<!--BOOK_TITLE-->", &title)
        .replace("<!--BOOK_LANGUAGE-->", &language)
        .replace("<!--BOOK_DESCRIPTION-->", &description_html)
        .replace("<!--BOOK_CREDITS-->", &credits)
        .replace("<!--PRIMARY_ACTION-->", &action)
        .replace("<!--LIBRARY_ACTION-->", &library_action)
        .replace(
            "<!--BATCH_PURCHASE_ACTION-->",
            &if batch_enabled {
                format!(
                    "<a class=\"ui-button ui-button--ghost\" href=\"/me/books/{}/buy-chapters\">Купить несколько глав</a>",
                    book.id
                )
            } else {
                String::new()
            },
        )
        .replace("<!--CHAPTERS-->", toc.unwrap_or(""))
        .replace("<!--BOOK_DISCUSSION-->", &discussion);
    // The current release has no authorized SEO-owner/approval projection wired
    // to H. Keep real book pages noindex; the F policy controls future activation.
    let seo = seo_head::disabled_book_head(
        book.id,
        first_free.is_some(),
        &book.title,
        &meta_description,
    );
    layout_with_seo(&book.title, &meta_description, &content, &seo)
}

#[allow(dead_code)]
pub fn reader(chapter: &tor_reading::services::PublicChapterPage) -> Html<String> {
    reader_with_login_link(chapter, None)
}

/// The public reader can link to the existing authenticated completion route.
/// A link never creates demand, grants access, or assumes that an open was read.
pub fn reader_with_login_link(
    chapter: &tor_reading::services::PublicChapterPage,
    chapter_id: Option<u64>,
) -> Html<String> {
    let previous = chapter
        .previous_chapter_id
        .map(|id| format!("<a rel=\"prev\" href=\"/reader/{id}\">Предыдущая глава</a>"))
        .unwrap_or_default();
    use tor_reading::services::PublicContinuation;
    let (next, continuation_status) = match chapter.continuation() {
        PublicContinuation::ReadNow(id) => (
            format!("<a rel=\"next\" href=\"/reader/{id}\">Следующая глава</a>"),
            "",
        ),
        PublicContinuation::NeedsAccess(id) => (
            format!("<a rel=\"next\" href=\"/reader/{id}\">Следующая глава — платная</a>"),
            "<p class=\"reader-next-state\">Следующая глава опубликована. Для чтения может потребоваться покупка.</p>",
        ),
        PublicContinuation::WaitingForRelease => (
            String::new(),
            "<p class=\"reader-next-state\">Следующих опубликованных глав пока нет.</p>",
        ),
    };
    let return_to_reading = if matches!(
        chapter.continuation(),
        PublicContinuation::WaitingForRelease
    ) {
        chapter_id
            .map(|id| {
                format!(
                    "<p class=\"reader-next-state\"><a class=\"text-link\" href=\"/read/{id}\">Отметить главу прочитанной и попросить продолжение</a></p>"
                )
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let continuation_status = format!("{continuation_status}{return_to_reading}");
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
        .replace("<!--CONTINUATION_STATUS-->", &continuation_status)
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
    static CSS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let mut css = String::from(include_str!("../assets/newboo-port.css"));
        css.push('\n');
        css.push_str(include_str!("../assets/newboo-home-carousel.css"));
        css.push('\n');
        css.push_str(include_str!("../assets/newboo-book-detail.css"));
        css.push('\n');
        css.push_str(include_str!("../assets/newboo-comments.css"));
        css.push('\n');
        css.push_str(include_str!("../assets/newboo-reader.css"));
        css.push('\n');
        css.push_str(include_str!("../assets/newboo-cover-picker.css"));
        css.push('\n');
        css.push_str(include_str!("../assets/newboo-layer-storefront.css"));
        css
    });
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        CSS.as_str(),
    )
}

/// Public static runtime only; no signed URL or scoped key is embedded here.
pub async fn paid_reader_script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/newboo-paid-reader.mjs"),
    )
}

/// Browser-only Layer CURRENT reader. Imports the existing paid WebCrypto module.
pub async fn layer_reader_script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/newboo-layer-reader.mjs"),
    )
}

/// Local cover-selection preview, based on NEwBOO book-settings.js.
pub async fn cover_picker_script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/newboo-cover-picker.js"),
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
use public_catalog::catalog_search;

#[cfg(test)]
pub fn book_detail_with_chapters(
    book: &tor_reading::services::PublicBookPage,
    limit: u16,
) -> Html<String> {
    book_detail_with_chapters_cover(book, limit, false)
}

#[cfg(test)]
mod tests;
