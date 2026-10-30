//! Actual author book/chapter pages: no placeholder books or fabricated chapters.
use axum::response::Html;
use std::fmt::Write;
use tor_books::services::authoring::{
    ChapterAccess, OwnedBook, OwnedChapter, OwnedChapterCursor, OwnedChapterSummary, PendingChapter,
};

use crate::pages::{escape_html, layout};

/// The form exists only when a signed source store and cover worker are enabled.
#[cfg(test)]
pub fn detail_with_covers(
    book: &OwnedBook,
    published: bool,
    chapters: &[OwnedChapterSummary],
    csrf: &str,
    uploads_enabled: bool,
    latest_job: Option<&str>,
) -> Html<String> {
    detail_with_covers_with_demand(
        book,
        published,
        chapters,
        csrf,
        uploads_enabled,
        latest_job,
        0,
    )
}

pub fn detail_with_covers_with_demand(
    book: &OwnedBook,
    published: bool,
    chapters: &[OwnedChapterSummary],
    csrf: &str,
    uploads_enabled: bool,
    latest_job: Option<&str>,
    demand_count: i64,
) -> Html<String> {
    detail_with_covers_with_demand_page(
        book,
        published,
        chapters,
        csrf,
        OwnerChapterPageOptions {
            uploads_enabled,
            latest_job,
            demand_count,
            next: None,
            has_previous_page: false,
        },
    )
}

/// All presentation controls for a bounded, owner-only chapter list.
pub struct OwnerChapterPageOptions<'a> {
    pub uploads_enabled: bool,
    pub latest_job: Option<&'a str>,
    pub demand_count: i64,
    pub next: Option<OwnedChapterCursor>,
    pub has_previous_page: bool,
}

/// Owner-only, keyset-paged list. Public book pages retain their own TOC.
pub fn detail_with_covers_with_demand_page(
    book: &OwnedBook,
    published: bool,
    chapters: &[OwnedChapterSummary],
    csrf: &str,
    options: OwnerChapterPageOptions<'_>,
) -> Html<String> {
    let OwnerChapterPageOptions {
        uploads_enabled,
        latest_job,
        demand_count,
        next,
        has_previous_page,
    } = options;
    let mut items = String::new();
    if chapters.is_empty() {
        items.push_str("<p class=\"empty-state\">В этой книге пока нет глав.</p>");
    } else {
        items.push_str("<ol>");
        for chapter in chapters {
            let access = match chapter.access {
                ChapterAccess::Free => "Бесплатная",
                ChapterAccess::Paid => "Платная",
            };
            let pending = published && !chapter.published;
            let edit_action = if chapter.access == ChapterAccess::Free {
                format!(
                    " <a href=\"/me/books/{}/chapters/{}/edit\">Редактировать</a>",
                    book.id, chapter.id
                )
            } else {
                String::new()
            };
            let release_action = if pending && chapter.access == ChapterAccess::Free {
                format!(
                    " <a href=\"/me/books/{}/chapters/{}/release\">Выпустить главу</a>",
                    book.id, chapter.id
                )
            } else {
                String::new()
            };
            let state = if pending {
                " — Черновик (не виден читателям)"
            } else {
                ""
            };
            write!(
                &mut items,
                "<li>№{} — {} ({access}){state}{edit_action}{release_action}</li>",
                chapter.number,
                escape_html(&chapter.title)
            )
            .expect("write String");
        }
        items.push_str("</ol>");
    }
    if has_previous_page || next.is_some() {
        items.push_str("<nav class=\"chapter-toc__more\" aria-label=\"Страницы глав\">");
        if has_previous_page {
            write!(
                &mut items,
                "<a class=\"button button--small\" href=\"/me/books/{}\">К первым главам</a> ",
                book.id
            )
            .expect("write String");
        }
        if let Some(cursor) = next {
            write!(
                &mut items,
                "<a class=\"button button--small\" href=\"/me/books/{}?after={}:{}\">Следующие главы</a>",
                book.id, cursor.number, cursor.id
            )
            .expect("write String");
        }
        items.push_str("</nav>");
    }

    let status = if published {
        "Опубликована"
    } else {
        "Черновик — книгу видит только владелец"
    };
    let action = if published {
        format!(
            "<p><a class=\"button button--quiet\" href=\"/books/{}\">Открыть опубликованную книгу</a></p>",
            book.id
        )
    } else if chapters.is_empty() {
        "<p>Сначала добавьте бесплатную главу, затем опубликуйте книгу.</p>".to_owned()
    } else {
        format!(
            r#"<form method="post" action="/me/books/{}/publish">
               <input type="hidden" name="csrf_token" value="{}">
               <button class="button" type="submit">Опубликовать книгу</button>
               </form>"#,
            book.id,
            escape_html(csrf)
        )
    };
    let add_action = format!(
        "<p><a class=\"button button--quiet\" href=\"/me/books/{}/chapters/new\">Добавить главу</a></p>",
        book.id
    );
    let cover_status = cover_status(latest_job);
    let upload = if uploads_enabled {
        format!(
            r#"<section class="author-cover-link" aria-label="Обложки книги">
                <h2>Обложки книги</h2>
                <p class="account-profile__hint">{cover_status}</p>
                <a class="ui-button ui-button--ghost" href="/me/books/{}/settings/covers">Управлять обложкой</a>
                </section>"#,
            book.id,
        )
    } else {
        String::new()
    };
    let demand_panel = if published {
        format!(
            "<section class=\"author-demand\" aria-label=\"Ожидание продолжения\"><h2>Читатели ждут продолжения</h2><p>Запросов после последней опубликованной главы: <strong>{demand_count}</strong></p><p class=\"account-profile__hint\">Только подтверждённые читатели текущей границы книги.</p></section>"
        )
    } else {
        String::new()
    };
    let body = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-chapter-card">
        <p class="eyebrow">Кабинет автора · книга №{}</p>
        <h1>{}</h1>
        <p class="account-profile__hint">{status}</p>
        {demand_panel}
        {upload}
        <p><a href="/me/books/{id}/description">Редактировать аннотацию</a></p>
        <h2>Главы</h2>{items}
        {add_action}
        {action}
        <p><a href="/me/books">Мои книги</a></p>
        </div></div></section>"#,
        book.id,
        escape_html(&book.title),
        id = book.id
    );
    layout(&book.title, "Управление своей книгой", &body)
}

fn cover_status(latest_job: Option<&str>) -> &'static str {
    match latest_job {
        Some("pending") => "Обложка ожидает обработки.",
        Some("running") | Some("ready") => "Обложка обрабатывается.",
        Some("done") => "Обложка готова.",
        Some("superseded") => "Обложка заменена более новой версией.",
        Some("dead") => "Ошибка обработки обложки. Загрузите другой файл.",
        _ => "Обложка ещё не загружена.",
    }
}

/// Mirrors NEwBOO's covers settings template and picker, one real TOR slot.
pub fn cover_settings(
    book: &OwnedBook,
    csrf: &str,
    latest_job: Option<&str>,
    current_public_cover: bool,
) -> Html<String> {
    let title = escape_html(&book.title);
    let image = if current_public_cover {
        format!(
            r#"<img class="book-cover-picker__image" src="/covers/{}/main.webp" alt="Обложка книги «{title}»" loading="lazy" data-cover-picker-image>"#,
            book.id
        )
    } else {
        r#"<img class="book-cover-picker__image" alt="Предварительный просмотр выбранной обложки" data-cover-picker-image hidden>"#
            .to_owned()
    };
    let html = include_str!("../templates/author_cover_settings.html")
        .replace("<!--BOOK_TITLE-->", &title)
        .replace("<!--BOOK_ID-->", &book.id.to_string())
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COVER_STATUS-->", cover_status(latest_job))
        .replace("<!--COVER_IMAGE-->", &image)
        .replace(
            "<!--PLACEHOLDER_HIDDEN-->",
            if current_public_cover { "hidden" } else { "" },
        )
        .replace(
            "<!--CHANGE_LABEL-->",
            if current_public_cover {
                "Заменить"
            } else {
                "Добавить"
            },
        )
        .replace(
            "<!--FILE_LABEL-->",
            if current_public_cover {
                "Текущая обложка"
            } else {
                "Файл не выбран"
            },
        );
    layout("Обложки книги", "Изменение обложки своей книги.", &html)
}

pub fn new_chapter(
    book_id: i64,
    csrf: &str,
    command_key: &str,
    paid_enabled: bool,
) -> Html<String> {
    // No paid-control is presented unless the private S3 writer and
    // owner-only publishing flow are explicitly enabled for this book.
    let access_fields = if paid_enabled {
        r#"<label for="chapter-access">Доступ к главе</label>
        <select id="chapter-access" name="access">
          <option value="free">Бесплатная</option>
          <option value="paid">Платная</option>
        </select>
        <label for="chapter-price">Цена в копейках (100 = 1 ₽; для бесплатной — 0)</label>
        <input id="chapter-price" name="price_amount" type="number" min="0" step="1" value="0" required>
        <p class="account-profile__hint">Платная глава будет опубликована только после проверки зашифрованного файла в закрытом хранилище. При ошибке сохранения повторите отправку той же формы.</p>"#
    } else {
        r#"<input type="hidden" name="access" value="free">
        <input type="hidden" name="price_amount" value="0">"#
    };
    let illustrated_link = if paid_enabled {
        format!(
            "<p><a href=\"/me/books/{book_id}/chapters/illustrated\">Добавить платную главу с иллюстрацией</a></p>"
        )
    } else {
        String::new()
    };
    let html = include_str!("../templates/author_new_chapter.html")
        .replace("<!--BOOK_ID-->", &book_id.to_string())
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key))
        .replace("<!--CHAPTER_ACCESS-->", access_fields)
        .replace(
            "<!--CHAPTER_HEADING-->",
            if paid_enabled { "Новая глава" } else { "Новая бесплатная глава" },
        )
        .replace(
            "<!--CHAPTER_INFO-->",
            if paid_enabled {
                "В опубликованной книге новая глава появляется у читателей сразу после сохранения. Платная глава дополнительно проверяется в защищённом хранилище."
            } else {
                "Если книга уже опубликована, новая глава появится у читателей сразу после сохранения. Для неопубликованной книги глава останется скрытой до выпуска книги."
            },
        )
        .replace("<!--SAVE_LABEL-->", "Сохранить главу")
        .replace("<!--PAID_IMAGE_LINK-->", &illustrated_link);
    layout("Добавить главу", "Публикация главы автором", &html)
}

/// Initial end-to-end paid author scenario: one image at the end of the
/// chapter. Browser never sees S3 keys; server freezes its asset ID with B text.
pub fn illustrated_chapter(book_id: i64, csrf: &str, command_key: &str) -> Html<String> {
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-chapter-card">
          <p class="eyebrow">Кабинет автора · книга №{book_id}</p>
          <h1>Платная глава с иллюстрацией</h1>
          <p>Одно изображение появится в конце главы. После публикации менять его нельзя: новая редакция создаётся отдельно.</p>
          <form class="auth-form author-form" action="/me/books/{book_id}/chapters/illustrated" method="post" enctype="multipart/form-data" autocomplete="off">
            <input type="hidden" name="csrf_token" value="{}">
            <input type="hidden" name="command_key" value="{}">
            <label for="illustrated-number">Номер главы</label>
            <input id="illustrated-number" name="number" type="number" min="1" max="1000000" value="1" required>
            <label for="illustrated-title">Название</label>
            <input id="illustrated-title" name="title" type="text" maxlength="200" required>
            <label for="illustrated-price">Цена в копейках</label>
            <input id="illustrated-price" name="price_amount" type="number" min="1" step="1" value="100" required>
            <label for="illustrated-body">Текст главы (Markdown, до 50 000 символов)</label>
            <textarea id="illustrated-body" name="body" rows="14" maxlength="49500" required></textarea>
            <label for="illustrated-file">Иллюстрация PNG, JPEG или WebP (до 8 МиБ)</label>
            <input id="illustrated-file" type="file" name="illustration"
                   accept="image/png,image/jpeg,image/webp,.png,.jpg,.jpeg,.webp" required>
            <p class="account-profile__hint">Изображение проверяется и шифруется отдельно. При потере ответа повторите отправку без изменения файла и текста.</p>
            <button class="button" type="submit">Сохранить платную главу с иллюстрацией</button>
          </form>
          <p class="auth-card__footer"><a href="/me/books/{book_id}/chapters/new">Вернуться к обычной форме</a></p>
        </div></div></section>"#,
        escape_html(csrf),
        escape_html(command_key),
    );
    layout(
        "Платная глава с иллюстрацией",
        "Создание зашифрованной редакции с изображением по образцу NEwBOO.",
        &content,
    )
}

pub fn edit_chapter(
    book_id: i64,
    chapter_id: i64,
    chapter: &OwnedChapter,
    csrf: &str,
    command_key: &str,
) -> Html<String> {
    edit_chapter_mode(book_id, chapter_id, chapter, csrf, command_key, false)
}

/// A published free chapter uses the same NEwBOO-style editor and save action,
/// but its new version is made readable atomically on the first POST.
pub fn edit_published_chapter(
    book_id: i64,
    chapter_id: i64,
    chapter: &OwnedChapter,
    csrf: &str,
    command_key: &str,
) -> Html<String> {
    edit_chapter_mode(book_id, chapter_id, chapter, csrf, command_key, true)
}

fn edit_chapter_mode(
    book_id: i64,
    chapter_id: i64,
    chapter: &OwnedChapter,
    csrf: &str,
    command_key: &str,
    published: bool,
) -> Html<String> {
    let description = if published {
        "Сохранение сразу обновит опубликованную редакцию. Предыдущая версия останется в истории."
    } else {
        "Редакция остаётся закрытой до выпуска книги или главы."
    };
    // NEwBOO has a "В главу" action. The TOR title itself opens the
    // published reader view; a draft has no publicly readable URL.
    let title_in_heading = if published {
        format!(
            "<a class=\"author-chapter-view-link\" href=\"/reader/{chapter_id}\" title=\"Открыть главу для чтения\">{}</a>",
            escape_html(&chapter.title)
        )
    } else {
        escape_html(&chapter.title)
    };
    let html = include_str!("../templates/author_edit_chapter.html")
        .replace(
            "<!--EDIT_EYEBROW-->",
            if published {
                "редактирование опубликованной главы"
            } else {
                "редактирование черновика"
            },
        )
        .replace("<!--EDIT_DESCRIPTION-->", description)
        .replace("<!--BOOK_ID-->", &book_id.to_string())
        .replace("<!--CHAPTER_ID-->", &chapter_id.to_string())
        .replace("<!--REVISION-->", &chapter.revision.to_string())
        .replace("<!--NUMBER-->", &chapter.number.to_string())
        .replace("<!--TITLE_HEADING-->", &title_in_heading)
        .replace("<!--TITLE-->", &escape_html(&chapter.title))
        .replace("<!--BODY-->", &escape_html(&chapter.body))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    layout("Редактировать главу", "Закрытый редактор черновика.", &html)
}

/// Confirmation is a separate page; the text and immutable version are shown
/// to the owner immediately before an explicit publication POST.
pub fn release_chapter(
    book_id: i64,
    chapter_id: i64,
    chapter: &PendingChapter,
    csrf: &str,
) -> Html<String> {
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-chapter-card">
        <p class="eyebrow">Кабинет автора · книга №{book_id}</p>
        <h1>Выпустить главу №{}</h1>
        <h2>{}</h2>
        <p>Проверяется редакция №{} (версия №{}). После выпуска эту версию смогут читать все.</p>
        <pre class="author-release-preview">{}</pre>
        <form method="post" action="/me/books/{book_id}/chapters/{chapter_id}/release">
          <input type="hidden" name="csrf_token" value="{}">
          <input type="hidden" name="expected_revision" value="{}">
          <input type="hidden" name="version_id" value="{}">
          <button class="button" type="submit">Опубликовать эту главу</button>
        </form>
        <p><a href="/me/books/{book_id}">Вернуться к черновикам</a></p>
        </div></div></section>"#,
        chapter.number,
        escape_html(&chapter.title),
        chapter.revision,
        chapter.version_id,
        escape_html(&chapter.body),
        escape_html(csrf),
        chapter.revision,
        chapter.version_id,
    );
    layout(
        "Выпустить главу",
        "Подтверждение публикации выбранной версии.",
        &content,
    )
}

#[cfg(test)]
pub fn detail(
    book: &OwnedBook,
    published: bool,
    chapters: &[OwnedChapterSummary],
    csrf: &str,
) -> Html<String> {
    detail_with_covers(book, published, chapters, csrf, false, None)
}

#[cfg(test)]
mod tests {
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
    use super::*;
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
}
