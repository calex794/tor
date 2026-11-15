//! Actual author book/chapter pages: no placeholder books or fabricated chapters.
use axum::response::Html;
use tor_books::schemas::ScheduledChapter;
use tor_books::services::authoring::{
    ChapterAccess, OwnedBook, OwnedChapter, OwnedChapterCursor, OwnedChapterSummary, PendingChapter,
};

use crate::pages::{escape_html, private_account_layout as layout};

mod book_workspace;
pub use book_workspace::detail_with_covers_with_demand_page;

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

#[cfg(test)]
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
            paid_editor_enabled: false,
            age_editor_enabled: false,
            next: None,
            has_previous_page: false,
        },
    )
}

pub struct OwnerChapterPageOptions<'a> {
    pub uploads_enabled: bool,
    pub latest_job: Option<&'a str>,
    pub demand_count: i64,
    pub paid_editor_enabled: bool,
    pub age_editor_enabled: bool,
    pub next: Option<OwnedChapterCursor>,
    pub has_previous_page: bool,
}
fn cover_status(latest_job: Option<&str>) -> &'static str {
    match latest_job {
        Some("pending") => "Обложка ожидает обработки.",
        Some("running") | Some("ready") => "Обложка обрабатывается.",
        Some("done") | Some("moderation_pending") => "Обложка обработана и ожидает модератора.",
        Some("moderation_approved") => "Обложка одобрена модератором.",
        Some("moderation_rejected") => "Модератор отклонил обложку. Загрузите другой файл.",
        Some("moderation_superseded") => "Ожидание решения по новой обложке.",
        Some("moderation_legacy") => "Обложка опубликована до введения модерации.",
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
/// H-09: only the author sees a private, unpublished-book import preview.
pub fn import_chapters(book_id: i64, csrf: &str, command_key: &str) -> Html<String> {
    let html = include_str!("../templates/author_import_chapters.html")
        .replace("<!--BOOK_ID-->", &book_id.to_string())
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    layout(
        "Импорт глав из файла",
        "Проверка файла и явное подтверждение без автоматической публикации.",
        &html,
    )
}

pub fn new_chapter(
    book_id: i64,
    csrf: &str,
    command_key: &str,
    paid_enabled: bool,
    is_public: bool,
    next_number: i32,
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
        <p class="account-profile__hint">Платная глава сначала шифруется и проверяется в закрытом хранилище. При отложенном выпуске она остаётся недоступной до слота расписания книги.</p>"#
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
        .replace("<!--NEXT_NUMBER-->", &next_number.to_string())
        .replace("<!--CHAPTER_ACCESS-->", access_fields)
        .replace(
            "<!--SCHEDULE_OPTION-->",
            if is_public {
                r#"<label><input type="checkbox" name="is_delayed_publication" value="1">
                   <strong>Отложенная публикация</strong></label>
                   <p class="account-profile__hint">Для бесплатной главы после сохранения назначьте UTC-время.
                   Для платной главы заранее включите расписание книги:
                   шифрованная версия будет открыта только в ближайший слот.</p>"#
            } else {
                ""
            },
        )
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
        .replace("<!--PAID_IMAGE_LINK-->", &illustrated_link)
        .replace(
            "<!--ACCOUNT_NAVIGATION-->",
            &crate::pages::account_navigation(&format!("/me/books/{book_id}/chapters/new"), csrf),
        );
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
    edit_chapter_mode(
        book_id,
        chapter_id,
        chapter,
        csrf,
        command_key,
        false,
        false,
    )
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
    edit_chapter_mode(book_id, chapter_id, chapter, csrf, command_key, true, false)
}

/// Verified paid editor: metadata is frozen; Save publishes encrypted V2.
pub fn edit_published_paid_chapter(
    book_id: i64,
    chapter_id: i64,
    chapter: &OwnedChapter,
    csrf: &str,
    command_key: &str,
) -> Html<String> {
    edit_chapter_mode(book_id, chapter_id, chapter, csrf, command_key, true, true)
}

fn edit_chapter_mode(
    book_id: i64,
    chapter_id: i64,
    chapter: &OwnedChapter,
    csrf: &str,
    command_key: &str,
    published: bool,
    paid: bool,
) -> Html<String> {
    let description = if paid {
        "После сохранения новая платная редакция появится только после проверки закрытого S3. Старая купленная редакция останется доступной. Номер, название и цена не изменяются."
    } else if published {
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
        .replace("<!--TITLE_READONLY-->", if paid { "readonly" } else { "" })
        .replace(
            "<!--EDIT_PRICE-->",
            if paid {
                format!("<p>Цена неизменна: {} копеек.</p>", chapter.price_amount)
            } else {
                String::new()
            }
            .as_str(),
        )
        .replace("<!--BODY-->", &escape_html(&chapter.body))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key))
        .replace(
            "<!--ACCOUNT_NAVIGATION-->",
            &crate::pages::account_navigation(&format!("/me/books/{book_id}/chapters/{chapter_id}/edit"), csrf),
        );
    layout("Редактировать главу", "Закрытый редактор черновика.", &html)
}

/// Confirmation is a separate page; the text and immutable version are shown
/// to the owner immediately before an explicit publication POST.
pub fn release_chapter(
    book_id: i64,
    chapter_id: i64,
    chapter: &PendingChapter,
    csrf: &str,
    schedule: Option<&ScheduledChapter>,
) -> Html<String> {
    let schedule_status = match schedule {
        Some(s) => format!(
            "<p>Расписание: {} UTC · статус: {}</p>",
            escape_html(&s.scheduled_utc),
            escape_html(&s.status)
        ),
        None => String::new(),
    };
    let cancel_form = if schedule.is_some_and(|s| s.status == "pending" || s.status == "stale") {
        format!(
            r#"<form method="post" action="/me/books/{book_id}/chapters/{chapter_id}/cancel-schedule">
               <input type="hidden" name="csrf_token" value="{}">
               <button class="button" type="submit">Отменить отложенный выпуск</button>
               </form>"#,
            escape_html(csrf)
        )
    } else {
        String::new()
    };
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
        <h3>Отложенный выпуск</h3>
        <p>Время указывается в UTC, а не в часовом поясе браузера.</p>
        {schedule_status}
        <form method="post" action="/me/books/{book_id}/chapters/{chapter_id}/schedule">
          <input type="hidden" name="csrf_token" value="{}">
          <input type="hidden" name="expected_revision" value="{}">
          <input type="hidden" name="version_id" value="{}">
          <label for="scheduled_utc">Дата и время (UTC)</label>
          <input id="scheduled_utc" type="datetime-local" name="scheduled_utc" required>
          <button class="button" type="submit">Запланировать или перенести выпуск</button>
        </form>
        {cancel_form}
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
mod tests;
