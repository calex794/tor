//! Owner-only book workspace: NEwBOO-style cover, status, chapters and grouped tools.
//! All actions remain existing TOR routes with their original server-side guards.
use axum::response::Html;
use std::fmt::Write;
use tor_books::services::authoring::{ChapterAccess, OwnedBook, OwnedChapterSummary};

use super::{cover_status, OwnerChapterPageOptions};
use crate::pages::{escape_html, private_account_layout as layout};

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
    let id = book.id;
    let title = escape_html(&book.title);
    let status_class = if published {
        "author-dashboard__badge--published"
    } else {
        "author-dashboard__badge--draft"
    };
    let status_label = if published {
        "Опубликована"
    } else {
        "Черновик"
    };
    let status_description = if published {
        "Книга опубликована. Управляйте главами и настройками, не покидая кабинет автора."
    } else {
        "Книга пока видна только владельцу. Добавьте главы и проверьте сведения перед публикацией."
    };
    // The latest processing job is not a reliable inventory of earlier covers.
    // Never request an unapproved private upload from the public cover endpoint.
    let cover_art = if matches!(
        latest_job,
        Some("moderation_approved" | "moderation_legacy")
    ) {
        format!(
            r#"<img class="author-dashboard__cover-image" src="/covers/{id}/main.webp" alt="Обложка книги «{title}»" loading="lazy">"#
        )
    } else {
        r#"<div class="author-dashboard__cover-placeholder" role="img" aria-label="Предпросмотр обложки">
<span class="author-dashboard__cover-symbol" aria-hidden="true">▤</span><span>Обложка книги</span>
</div>"#
            .to_owned()
    };
    let cover_action = if uploads_enabled {
        format!(
            r#"<a class="author-dashboard__cover-link" href="/me/books/{id}/settings/covers">Управлять обложкой</a>
<p class="author-dashboard__cover-status">{}</p>"#,
            cover_status(latest_job)
        )
    } else {
        String::new()
    };
    let hero_actions = if published {
        format!(
            r##"<a class="ui-button ui-button--primary" href="/books/{id}">Открыть книгу</a>
        <a class="ui-button ui-button--ghost" href="#chapters">Управлять главами</a>"##
        )
    } else {
        format!(
            r##"<a class="ui-button ui-button--primary" href="/me/books/{id}/chapters/new">Добавить главу</a>
        <a class="ui-button ui-button--ghost" href="#preparation">Подготовка к публикации</a>"##
        )
    };

    let preparation = if published {
        String::new()
    } else {
        let chapter_progress = if chapters.is_empty() {
            r#"<span class="author-dashboard__step-status">Ожидает добавления</span>"#
        } else {
            r#"<span class="author-dashboard__step-status">Главы добавлены</span>"#
        };
        let publish_action = if chapters.is_empty() {
            r#"<p class="author-dashboard__publish-note">Сначала добавьте бесплатную главу, затем опубликуйте книгу.</p>"#
                .to_owned()
        } else {
            format!(
                r#"<form class="author-dashboard__publish-form" method="post" action="/me/books/{id}/publish">
<input type="hidden" name="csrf_token" value="{}">
<button class="ui-button ui-button--primary" type="submit">Опубликовать книгу</button>
</form><p class="author-dashboard__publish-note">Перед публикацией проверьте, что в книге есть бесплатная глава. Это проверяется на сервере.</p>"#,
                escape_html(csrf)
            )
        };
        let cover_step = if uploads_enabled {
            format!(
                r#"<li class="author-dashboard__step author-dashboard__step--optional">
<span class="author-dashboard__step-mark" aria-hidden="true">＋</span>
<span><strong>Обложка <span class="author-dashboard__optional">· рекомендуется</span></strong>
<a href="/me/books/{id}/settings/covers">Проверить оформление</a></span></li>"#
            )
        } else {
            String::new()
        };
        format!(
            r#"<section class="author-dashboard__panel author-dashboard__preparation" id="preparation" aria-labelledby="author-preparation-title">
<div class="author-dashboard__panel-heading"><div><span class="author-dashboard__kicker">Следующий шаг</span>
<h2 id="author-preparation-title">Подготовка к публикации</h2></div>
<span class="author-dashboard__badge author-dashboard__badge--neutral">Только владелец</span></div>
<ol class="author-dashboard__steps">
<li class="author-dashboard__step author-dashboard__step--complete">
<span class="author-dashboard__step-mark" aria-hidden="true">✓</span>
<span><strong>Название книги</strong><span class="author-dashboard__step-status">Указано</span></span></li>
<li class="author-dashboard__step">
<span class="author-dashboard__step-mark" aria-hidden="true">2</span>
<span><strong>Первая бесплатная глава</strong>{chapter_progress}
<a href="/me/books/{id}/chapters/new">Добавить или проверить главы</a></span></li>
<li class="author-dashboard__step author-dashboard__step--optional">
<span class="author-dashboard__step-mark" aria-hidden="true">3</span>
<span><strong>Аннотация и сведения</strong><a href="/me/books/{id}/description">Проверить аннотацию</a></span></li>
{cover_step}
</ol>
<div class="author-dashboard__publish-actions">{publish_action}</div>
</section>"#
        )
    };
    let demand_panel = if published {
        format!(
            r#"<section class="author-dashboard__demand" aria-label="Ожидание продолжения">
<h2>Читатели ждут продолжения</h2>
<p>Запросов после последней опубликованной главы: <strong>{demand_count}</strong></p>
<p>Только подтверждённые читатели текущей границы книги.</p></section>"#
        )
    } else {
        String::new()
    };

    let mut items = String::new();
    if chapters.is_empty() {
        write!(
            &mut items,
            r#"<div class="author-dashboard__empty">
<h3>В этой книге пока нет глав.</h3>
<p>Начните с бесплатной главы — после этого книгу можно будет подготовить к публикации.</p>
<a class="ui-button ui-button--primary" href="/me/books/{id}/chapters/new">Добавить первую главу</a>
</div>"#
        )
        .expect("write String");
    } else {
        items.push_str(r#"<ol class="author-dashboard__chapters">"#);
        for chapter in chapters {
            let access = match chapter.access {
                ChapterAccess::Free => "Бесплатная",
                ChapterAccess::Paid => "Платная",
            };
            let state = if !published {
                "В черновике книги"
            } else if chapter.published {
                "Выпущена"
            } else {
                "Черновик — не видна читателям"
            };
            let edit_action = if chapter.access == ChapterAccess::Free
                || (chapter.access == ChapterAccess::Paid
                    && chapter.published
                    && paid_editor_enabled)
            {
                format!(
                    r#"<a href="/me/books/{id}/chapters/{}/edit">Редактировать</a>"#,
                    chapter.id
                )
            } else {
                String::new()
            };
            let release_action =
                if published && !chapter.published && chapter.access == ChapterAccess::Free {
                    format!(
                        r#"<a href="/me/books/{id}/chapters/{}/release">Выпустить главу</a>"#,
                        chapter.id
                    )
                } else {
                    String::new()
                };
            write!(
                &mut items,
                r#"<li class="author-dashboard__chapter">
<div class="author-dashboard__chapter-main"><span class="author-dashboard__chapter-number">№{}</span>
<strong>{}</strong></div>
<div class="author-dashboard__chapter-meta"><span>{access}</span><span>{state}</span></div>
<div class="author-dashboard__chapter-actions">{edit_action}{release_action}</div>
</li>"#,
                chapter.number,
                escape_html(&chapter.title)
            )
            .expect("write String");
        }
        items.push_str("</ol>");
    }
    let mut pagination = String::new();
    if has_previous_page || next.is_some() {
        pagination
            .push_str(r#"<nav class="author-dashboard__pagination" aria-label="Страницы глав">"#);
        if has_previous_page {
            write!(
                &mut pagination,
                r#"<a class="ui-button ui-button--ghost" href="/me/books/{id}">К первым главам</a>"#
            )
            .expect("write String");
        }
        if let Some(cursor) = next {
            write!(
                &mut pagination,
                r#"<a class="ui-button ui-button--ghost" href="/me/books/{id}?after={}:{}">Следующие главы</a>"#,
                cursor.number, cursor.id
            )
            .expect("write String");
        }
        pagination.push_str("</nav>");
    }
    let chapter_actions = if published {
        format!(
            r#"<a class="ui-button ui-button--primary" href="/me/books/{id}/chapters/new">Добавить главу</a>"#
        )
    } else {
        format!(
            r#"<a class="ui-button ui-button--primary" href="/me/books/{id}/chapters/new">Добавить главу</a>
<a class="ui-button ui-button--ghost" href="/me/books/{id}/chapters/import">Импорт глав из файла</a>"#
        )
    };
    let age_settings_link = if age_editor_enabled {
        format!(
            r#"<li><a href="/me/books/{id}/settings/access-level">Возрастной уровень книги (18+/21+)</a></li>"#
        )
    } else {
        String::new()
    };
    let chapter_count = if has_previous_page || pagination.contains("Следующие главы")
    {
        format!("На странице: {}", chapters.len())
    } else {
        format!("Глав: {}", chapters.len())
    };
    let content = include_str!("../../templates/author_book_dashboard.html")
        .replace("<!--BOOK_ID-->", &id.to_string())
        .replace("<!--BOOK_TITLE-->", &title)
        .replace("<!--STATUS_CLASS-->", status_class)
        .replace("<!--STATUS_LABEL-->", status_label)
        .replace("<!--STATUS_DESCRIPTION-->", status_description)
        .replace("<!--CHAPTER_COUNT-->", &chapter_count)
        .replace("<!--COVER_ART-->", &cover_art)
        .replace("<!--COVER_ACTION-->", &cover_action)
        .replace("<!--HERO_ACTIONS-->", &hero_actions)
        .replace("<!--PREPARATION_SECTION-->", &preparation)
        .replace("<!--DEMAND_SECTION-->", &demand_panel)
        .replace("<!--CHAPTER_ACTIONS-->", &chapter_actions)
        .replace("<!--CHAPTER_ITEMS-->", &items)
        .replace("<!--CHAPTER_PAGINATION-->", &pagination)
        .replace("<!--AGE_SETTINGS_LINK-->", &age_settings_link);
    layout(&book.title, "Управление своей книгой", &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> OwnedBook {
        OwnedBook {
            id: 42,
            title: r#"<script>alert("x")</script>"#.to_owned(),
        }
    }

    fn options() -> OwnerChapterPageOptions<'static> {
        OwnerChapterPageOptions {
            uploads_enabled: false,
            latest_job: None,
            demand_count: 0,
            paid_editor_enabled: false,
            age_editor_enabled: false,
            next: None,
            has_previous_page: false,
        }
    }

    #[test]
    fn draft_has_safe_guided_actions_and_no_public_book_link() {
        let html = detail_with_covers_with_demand_page(&book(), false, &[], "<csrf>", options()).0;
        assert!(html.contains("Подготовка к публикации"));
        assert!(html.contains("В этой книге пока нет глав."));
        assert!(html.contains("/me/books/42/chapters/import"));
        assert!(!html.contains("action=\"/me/books/42/publish\""));
        assert!(!html.contains("href=\"/books/42\""));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert"));
    }

    #[test]
    fn publish_stays_post_with_escaped_csrf() {
        let chapter = OwnedChapterSummary {
            id: 7,
            number: 1,
            title: "Первая".into(),
            access: ChapterAccess::Free,
            published: false,
        };
        let html =
            detail_with_covers_with_demand_page(&book(), false, &[chapter], "<csrf>", options()).0;
        assert!(html.contains("method=\"post\" action=\"/me/books/42/publish\""));
        assert!(html.contains("name=\"csrf_token\" value=\"&lt;csrf&gt;\""));
    }

    #[test]
    fn published_book_hides_import_and_publication_and_preserves_reader_link() {
        let chapter = OwnedChapterSummary {
            id: 9,
            number: 2,
            title: "Глава <x>".into(),
            access: ChapterAccess::Paid,
            published: true,
        };
        let html =
            detail_with_covers_with_demand_page(&book(), true, &[chapter], "csrf", options()).0;
        assert!(html.contains("href=\"/books/42\""));
        assert!(html.contains("Читатели ждут продолжения"));
        assert!(!html.contains("/chapters/import"));
        assert!(!html.contains("/chapters/9/edit"));
        assert!(!html.contains("action=\"/me/books/42/publish\""));
        assert!(html.contains("Глава &lt;x&gt;"));
    }

    #[test]
    fn owner_only_cover_controls_respect_feature_gate() {
        let mut enabled = options();
        enabled.uploads_enabled = true;
        enabled.latest_job = Some("pending");
        let page = detail_with_covers_with_demand_page(&book(), false, &[], "csrf", enabled).0;
        assert!(page.contains("/me/books/42/settings/covers"));
        assert!(page.contains("ожидает обработки"));
        assert!(!page.contains("/covers/42/main.webp"));
        let disabled =
            detail_with_covers_with_demand_page(&book(), false, &[], "csrf", options()).0;
        assert!(!disabled.contains("/settings/covers"));
    }
}
