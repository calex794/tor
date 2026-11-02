//! S011: bounded comments under the exact published free chapter edition.
//! Rendering never trusts stored HTML and does not fetch paid or old editions.
use super::escape_html;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_discussions::{
    domain::spoilers::{mask_mentions, parse, Segment},
    repositories::public_chapter_comments::{ChapterComment, ChapterCommentPage},
};

fn comment_body_html(body: &str) -> String {
    let mut out = String::new();
    for segment in parse(body) {
        match segment {
            Segment::Text(text) => out.push_str(&escape_html(&mask_mentions(text))),
            Segment::Spoiler(text) => {
                out.push_str("<details class=\"comment-spoiler\"><summary class=\"comment-spoiler__toggle\">Спойлер</summary><span class=\"comment-spoiler__content\">");
                out.push_str(&escape_html(&mask_mentions(text)));
                out.push_str("</span></details>");
            }
        }
    }
    out
}

fn submit_form(chapter_id: u64, version_id: u64, csrf: &str, parent: Option<i64>) -> String {
    let (label, extra) = match parent {
        Some(id) => (
            format!("Ваш ответ на комментарий №{id}"),
            format!("<input type=\"hidden\" name=\"parent_id\" value=\"{id}\">"),
        ),
        None => (
            "Комментарий к главе".to_owned(),
            "<input type=\"hidden\" data-chapter-comment-block>\
             <input type=\"hidden\" data-chapter-comment-start>\
             <input type=\"hidden\" data-chapter-comment-end>\
             <input type=\"hidden\" data-chapter-comment-quote>"
                .to_owned(),
        ),
    };
    let hint = if parent.is_none() {
        "<p class=\"reader-chapter-comments__anchor\" data-chapter-comment-anchor-status role=\"status\">Можно выделить фразу в тексте главы или написать общий комментарий.</p>"
    } else {
        ""
    };
    format!(
        "<form class=\"comments-form reader-chapter-comments__form\" \
         method=\"post\" action=\"/reader/{chapter_id}/comments\" \
         data-chapter-comment-form>\
         <label for=\"{field_id}\">{label}</label>\
         <p>Чтобы скрыть спойлер, заключите текст в ||двойные черты||.</p>\
         {hint}\
         <textarea name=\"body\" id=\"{field_id}\" rows=\"3\" maxlength=\"2000\" required \
         class=\"ui-textarea comments-form__input\" placeholder=\"Написать комментарий\"></textarea>\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
         <input type=\"hidden\" name=\"version_id\" value=\"{version_id}\">\
         <input type=\"hidden\" name=\"command_key\" value=\"{intent}\">\
         {extra}\
         <button class=\"ui-button ui-button--primary\" type=\"submit\">{button}</button>\
         </form>",
        field_id = if parent.is_some() {
            format!("chapter-reply-{}", parent.unwrap_or_default())
        } else {
            "chapter-comment-body".to_owned()
        },
        csrf = escape_html(csrf),
        intent = generate_session_token(),
        button = if parent.is_some() {
            "Отправить ответ"
        } else {
            "Отправить комментарий"
        },
        label = escape_html(&label),
        extra = extra,
        hint = hint,
    )
}

fn comment_card(chapter_id: u64, version_id: u64, csrf: Option<&str>, entry: &ChapterComment) -> String {
    let id = entry.id;
    let reply_ref = entry.parent_id.map_or_else(String::new, |parent| {
        format!("<p class=\"reader-chapter-comments__reply-ref\">Ответ на комментарий №{parent}</p>")
    });
    let selection = entry.anchor.as_ref().map_or_else(String::new, |anchor| {
        let block = anchor.block_index;
        let start = anchor.start_utf16;
        let end = anchor.end_utf16;
        format!(
            "<p><a href=\"#chapter-text\" class=\"reader-chapter-comments__goto\" \
             data-chapter-comment-goto data-anchor-block=\"{block}\" \
             data-anchor-start=\"{start}\" data-anchor-end=\"{end}\">\
             К фрагменту {block}</a></p>"
        )
    });
    let reply = if entry.parent_id.is_none() {
        csrf.map_or_else(String::new, |csrf| {
            format!(
                "<details class=\"comment-reply\"><summary>Ответить на комментарий №{id}</summary>\
                 {}</details>",
                submit_form(chapter_id, version_id, csrf, Some(id))
            )
        })
    } else {
        String::new()
    };
    format!(
        "<article class=\"comment-card\" id=\"chapter-comment-{id}\">\
         <div class=\"comment-card__head\"><span class=\"comment-card__author\">Читатель · комментарий №{id}</span></div>\
         {reply_ref}{selection}<div class=\"comment-card__body\">{body}</div>{reply}</article>",
        body = comment_body_html(&entry.body),
    )
}

/// The guest sees only public free comments. Only a verified session receives
/// an actor-bound form; all writes recheck the same edition in PostgreSQL.
pub(super) fn panel(
    chapter_id: Option<u64>,
    version_id: u64,
    csrf: Option<&str>,
    page: Option<&ChapterCommentPage>,
    feedback: Option<&str>,
) -> String {
    let Some(chapter_id) = chapter_id else {
        return String::new();
    };
    let Some(page) = page else {
        return String::new();
    };
    let status = if csrf.is_some() {
        match feedback {
            Some("sent") => "<p class=\"reader-chapter-comments__feedback\" role=\"status\">Комментарий опубликован.</p>",
            Some("repeat") => "<p class=\"reader-chapter-comments__feedback\" role=\"status\">Комментарий уже был опубликован.</p>",
            _ => "",
        }
    } else {
        ""
    };
    let form = csrf.map_or_else(
        || {
            format!(
                "<p class=\"reader-chapter-comments__login\"><a class=\"text-link\" href=\"/login?next=/reader/{chapter_id}\">Войти, чтобы оставить комментарий</a></p>"
            )
        },
        |csrf| submit_form(chapter_id, version_id, csrf, None),
    );
    let mut list = String::new();
    if page.items.is_empty() {
        list.push_str("<p class=\"comments-block__empty\">Комментариев к этой редакции пока нет.</p>");
    } else {
        list.push_str("<div class=\"comments-list\">");
        for entry in &page.items {
            list.push_str(&comment_card(chapter_id, version_id, csrf, entry));
        }
        list.push_str("</div>");
    }
    if let Some(cursor) = page.next_cursor {
        write!(
            list,
            "<nav class=\"reader-chapter-comments__pagination\" aria-label=\"Страницы комментариев\">\
             <a href=\"/reader/{chapter_id}?comments_after={cursor}#chapter-comments\">\
             Более ранние комментарии</a></nav>"
        )
        .expect("write comments pagination");
    }
    format!(
        "<section class=\"comments-block reader-chapter-comments\" id=\"chapter-comments\" \
         aria-labelledby=\"chapter-comments-title\">\
         <div class=\"comments-block__head\"><div>\
         <p class=\"comments-block__kicker\">Обсуждение редакции {version_id}</p>\
         <h2 class=\"comments-block__title\" id=\"chapter-comments-title\">Комментарии к главе</h2>\
         </div></div>{status}{form}{list}</section>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_discussions::repositories::public_chapter_comments::{
        ChapterCommentAnchor, ChapterCommentPage,
    };

    fn page() -> ChapterCommentPage {
        ChapterCommentPage {
            items: vec![ChapterComment {
                id: 7,
                parent_id: None,
                body: "<script>alert(1)</script> ||спойлер @unknown||".into(),
                anchor: Some(ChapterCommentAnchor {
                    block_index: 2,
                    start_utf16: 4,
                    end_utf16: 9,
                }),
            }],
            next_cursor: None,
        }
    }

    #[test]
    fn s011_guest_gets_public_exact_version_but_no_post_form_or_csrf() {
        let html = panel(Some(101), 602, None, Some(&page()), Some("sent"));
        assert!(html.contains("Обсуждение редакции 602"));
        assert!(html.contains("data-anchor-block=\"2\""));
        assert!(html.contains("data-anchor-start=\"4\""));
        assert!(html.contains("Войти, чтобы оставить комментарий"));
        assert!(!html.contains("name=\"csrf_token\""));
        assert!(!html.contains("Комментарий опубликован."));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("<details class=\"comment-spoiler\">"));
    }

    #[test]
    fn s011_actor_forms_bind_same_version_and_new_command_key() {
        let html = panel(Some(101), 602, Some("safe"), Some(&page()), Some("repeat"));
        assert!(html.contains("name=\"version_id\" value=\"602\""));
        assert!(html.contains("name=\"csrf_token\" value=\"safe\""));
        assert!(html.contains("name=\"command_key\" value=\""));
        assert!(html.contains("name=\"parent_id\" value=\"7\""));
        assert!(html.contains("Комментарий уже был опубликован."));
        assert!(html.contains("name=\"body\""));
        assert!(!html.contains("name=\"anchor_quote\""));
        assert!(!html.contains("data-anchor-chapter"));
        assert!(panel(None, 602, Some("safe"), Some(&page()), None).is_empty());
        assert!(panel(Some(101), 602, Some("safe"), None, None).is_empty());
    }

    #[test]
    fn s011_escaped_csrf_and_never_echo_untrusted_status() {
        let output = panel(Some(101), 602, Some("\" onmouseover=\"x"), Some(&page()), Some("<script>"));
        assert!(output.contains("&quot; onmouseover=&quot;x"));
        assert!(!output.contains("csrf_token\" value=\"\" onmouseover"));
        assert!(!output.contains("Комментарий уже был опубликован."));
    }
}
