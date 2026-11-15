//! Owner book workspace; rendering only, with the existing publication gates.
use super::{
    cover_status, escape_html, layout, ChapterAccess, OwnedBook, OwnedChapterCursor,
    OwnedChapterSummary, OwnerChapterPageOptions,
};
use axum::response::Html;
use std::fmt::Write;

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
        paid_editor_enabled,
        age_editor_enabled,
        next,
        has_previous_page,
    } = options;
    let publication = if published {
        format!(
            "<a class=\"ui-button ui-button--ghost\" href=\"/books/{}\">Открыть страницу книги</a>",
            book.id
        )
    } else if chapters.is_empty() {
        let hint = if has_previous_page {
            "Вернитесь к первым главам, чтобы опубликовать книгу."
        } else {
            "Сначала добавьте бесплатную главу, затем опубликуйте книгу."
        };
        format!("<p class=\"author-book-workspace__hint\">{hint}</p>")
    } else {
        format!(
            r#"<form class="author-book-workspace__publish" method="post" action="/me/books/{}/publish">
<input type="hidden" name="csrf_token" value="{}">
<button class="ui-button ui-button--ghost" type="submit">Опубликовать книгу</button></form>"#,
            book.id,
            escape_html(csrf)
        )
    };
    let import = if published {
        String::new()
    } else {
        format!("<p class=\"author-book-workspace__import\"><a class=\"text-link\" href=\"/me/books/{}/chapters/import\">Импорт глав из файла</a></p>", book.id)
    };
    let age_link = if age_editor_enabled {
        format!(
            "<a href=\"/me/books/{}/settings/access-level\">Возрастной уровень книги (18+/21+)</a>",
            book.id
        )
    } else {
        String::new()
    };
    let mut extras = String::new();
    if uploads_enabled {
        write!(extras, r#"<section class="author-book-workspace__panel" aria-label="Обложки книги">
<h2>Обложки книги</h2><p class="author-book-workspace__hint">{}</p>
<a class="ui-button ui-button--ghost" href="/me/books/{}/settings/covers">Управлять обложкой</a></section>"#,
            cover_status(latest_job), book.id).expect("write String");
    }
    if published {
        write!(extras, "<section class=\"author-book-workspace__panel\" aria-label=\"Ожидание продолжения\"><h2>Читатели ждут продолжения</h2><p>Запросов после последней опубликованной главы: <strong>{demand_count}</strong></p><p class=\"author-book-workspace__hint\">Только подтверждённые читатели текущей границы книги.</p></section>").expect("write String");
    }
    let (state, note) = if published {
        (
            "Публикация включена",
            "Доступ читателей зависит от проверки регистрации и действующих ограничений.",
        )
    } else {
        (
            "Черновик",
            "Книга ещё не открыта читателям. Сохранение глав и публикация — отдельные действия.",
        )
    };
    // A job's moderation outcome is not a complete inventory of older covers.
    // Never insert a private/pending source into the public <img> element.
    let cover_preview = if matches!(
        latest_job,
        Some("moderation_approved" | "moderation_legacy")
    ) {
        format!(
            r#"<img src="/covers/{}/main.webp" alt="Обложка книги «{}»" loading="lazy">"#,
            book.id,
            escape_html(&book.title)
        )
    } else {
        r#"<div class="author-book-workspace__cover-placeholder" role="img" aria-label="Обложка книги"><span aria-hidden="true">▤</span><small>Обложка книги</small></div>"#
            .to_owned()
    };
    // This view only knows the chapters on the current page, not a global
    // content checklist or registration/moderation readiness.
    let preparation_panel = if published {
        String::new()
    } else {
        let chapter_hint = if chapters.iter().any(|c| c.access == ChapterAccess::Free) {
            "Бесплатная глава есть на этой странице"
        } else if has_previous_page {
            "Проверьте бесплатные главы на предыдущих страницах"
        } else {
            "Добавьте хотя бы одну бесплатную главу"
        };
        format!(
            r#"<section class="author-book-workspace__panel author-book-workspace__preparation" aria-labelledby="author-book-preparation">
<div class="author-book-workspace__panel-heading">
<div><p class="author-book-workspace__kicker">Следующий шаг</p>
<h2 id="author-book-preparation">Подготовка к публикации</h2></div>
<span class="author-book-state is-draft">Только владелец</span>
</div>
<div class="author-book-workspace__steps">
<div class="author-book-workspace__step"><strong>1. Название книги</strong><span>Указано</span></div>
<div class="author-book-workspace__step"><strong>2. Бесплатная глава</strong>
<span>{chapter_hint}</span><a href="/me/books/{}/chapters/new">Создать главу</a></div>
<div class="author-book-workspace__step"><strong>3. Описание</strong>
<span>Проверьте перед публикацией</span><a href="/me/books/{}/description">Проверить описание</a></div>
</div>
<p class="author-book-workspace__hint">Состояние регистрации и возможность публикации проверяются на сервере. Здесь показаны только доступные сведения.</p>
</section>"#,
            book.id, book.id
        )
    };
    let path = format!("/me/books/{}", book.id);
    let body = include_str!("../../templates/author_book_workspace.html")
        .replace(
            "<!--ACCOUNT_NAVIGATION-->",
            &crate::pages::account_navigation(&path, csrf),
        )
        .replace("<!--BOOK_ID-->", &book.id.to_string())
        .replace("<!--BOOK_TITLE-->", &escape_html(&book.title))
        .replace("<!--COVER_PREVIEW-->", &cover_preview)
        .replace("<!--PREPARATION_PANEL-->", &preparation_panel)
        .replace(
            "<!--STATE_CLASS-->",
            if published {
                "is-published"
            } else {
                "is-draft"
            },
        )
        .replace("<!--BOOK_STATE-->", state)
        .replace("<!--STATE_NOTE-->", note)
        .replace("<!--PUBLICATION_ACTION-->", &publication)
        .replace("<!--CHAPTER_COUNT-->", &chapters.len().to_string())
        .replace(
            "<!--CHAPTER_LIST-->",
            &chapter_list(
                book.id,
                published,
                chapters,
                paid_editor_enabled,
                next,
                has_previous_page,
            ),
        )
        .replace("<!--IMPORT_ACTION-->", &import)
        .replace("<!--AGE_SETTINGS_LINK-->", &age_link)
        .replace("<!--EXTRA_PANELS-->", &extras);
    layout(&book.title, "Управление своей книгой", &body)
}

fn chapter_list(
    book: i64,
    published: bool,
    chapters: &[OwnedChapterSummary],
    paid_editor: bool,
    next: Option<OwnedChapterCursor>,
    has_previous: bool,
) -> String {
    let mut out = String::new();
    if chapters.is_empty() {
        let message = if has_previous {
            "На этой странице нет глав."
        } else {
            "В этой книге пока нет глав."
        };
        write!(out, "<p class=\"author-workspace__empty\">{message}</p>").expect("write String");
    } else {
        out.push_str("<ol class=\"author-chapter-list\">");
        for chapter in chapters {
            let pending = published && !chapter.published;
            let (access, state) = (
                match chapter.access {
                    ChapterAccess::Free => "Бесплатная",
                    ChapterAccess::Paid => "Платная",
                },
                if pending {
                    "Черновик (не виден читателям)"
                } else if !published {
                    "В черновике книги"
                } else {
                    "Редакция выпущена"
                },
            );
            write!(out, "<li class=\"author-chapter-row\"><span class=\"author-chapter-row__number\">№{}</span><div class=\"author-chapter-row__copy\"><h3>{}</h3><p><span>{access}</span><span>{state}</span></p></div><div class=\"author-chapter-row__actions\">", chapter.number, escape_html(&chapter.title)).expect("write String");
            if chapter.access == ChapterAccess::Free
                || (chapter.access == ChapterAccess::Paid && chapter.published && paid_editor)
            {
                write!(out, "<a class=\"ui-button ui-button--ghost\" href=\"/me/books/{book}/chapters/{}/edit\">Редактировать</a>", chapter.id).expect("write String");
            }
            if pending && chapter.access == ChapterAccess::Free {
                write!(out, "<a class=\"ui-button ui-button--ghost\" href=\"/me/books/{book}/chapters/{}/release\">Выпустить главу</a>", chapter.id).expect("write String");
            }
            out.push_str("</div></li>");
        }
        out.push_str("</ol>");
    }
    if has_previous || next.is_some() {
        out.push_str("<nav class=\"chapter-toc__more\" aria-label=\"Страницы глав\">");
        if has_previous {
            write!(out, "<a class=\"ui-button ui-button--ghost\" href=\"/me/books/{book}\">К первым главам</a>").expect("write String");
        }
        if let Some(cursor) = next {
            write!(out, "<a class=\"ui-button ui-button--ghost\" href=\"/me/books/{book}?after={}:{}\">Следующие главы</a>", cursor.number, cursor.id).expect("write String");
        }
        out.push_str("</nav>");
    }
    out
}
