use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_social::domain::{Conversation, MessagePage};

fn shell(body: &str) -> Html<String> {
    layout("Личная почта","Личные диалоги и сообщения.",&format!("<section class=\"me-page mail-page\"><div class=\"container\"><nav class=\"dashboard-nav\" aria-label=\"Разделы кабинета\"><a class=\"dashboard-nav__item\" href=\"/me\">Обзор</a><a class=\"dashboard-nav__item is-active\" href=\"/mail\">Личная почта</a><a class=\"dashboard-nav__item\" href=\"/me/inbox\">Обсуждения</a></nav>{body}</div></section>"))
}

pub(super) fn index(rows: &[(Conversation, String)], limit: u16) -> Html<String> {
    // Source index is a conversation selector, not a numeric-recipient composer.
    let mut body = String::from("<header class=\"ui-page-header\"><div><h1 class=\"ui-page-header__title\">Почта</h1><p>Выберите переписку</p></div></header><section class=\"me-section\" aria-label=\"Переписки\">");
    if rows.is_empty() {
        body.push_str("<p>Диалогов пока нет.</p>");
    }
    for (c, name) in rows {
        let blocked = if c.blocked {
            " · Новые сообщения заблокированы"
        } else {
            ""
        };
        let preview = if c.last_message_preview.is_empty() {
            "Сообщений пока нет"
        } else {
            &c.last_message_preview
        };
        write!(
            body,
            "<article class=\"me-now-card\"><a href=\"/mail/{}\">{}</a><time>{}</time><p class=\"mail-message-body\">{}</p><p>{blocked}</p></article>",
            c.id,
            escape_html(name),
            escape_html(&c.last_message_at),
            escape_html(preview)
        )
        .expect("mail markup");
    }
    if rows.len() == usize::from(limit) {
        if let Some((last, _)) = rows.last() {
            write!(
                body,
                "<a href=\"/mail?after_id={}&amp;limit={limit}\">Следующие диалоги</a>",
                last.id
            )
            .expect("mail cursor");
        }
    }
    body.push_str("</section>");
    shell(&body)
}

pub(super) fn conversation(
    actor: i64,
    id: i64,
    page: &MessagePage,
    peer_name: &str,
    csrf: &str,
    key: &str,
) -> Html<String> {
    let csrf = escape_html(csrf);
    let peer_name = escape_html(peer_name);
    let last = page.items.last().map_or(0, |m| m.id);
    let mut body=format!("<header class=\"ui-page-header\"><div><h1 class=\"ui-page-header__title\">{}</h1><p><a href=\"/mail\">Все диалоги</a></p></div></header><section class=\"me-section\"><p id=\"mail-status\" role=\"status\"></p><div id=\"mail-messages\" data-conversation=\"{id}\" data-last-id=\"{last}\" data-actor=\"{actor}\" data-peer-name=\"{peer_name}\" aria-live=\"polite\">",peer_name);
    for m in &page.items {
        let sender = if m.sender_id == actor {
            "Вы"
        } else {
            &peer_name
        };
        write!(body,"<article class=\"me-now-card\" data-message-id=\"{}\"><strong>{sender}</strong><p class=\"mail-message-body\">{}</p><time>{}</time></article>",m.id,escape_html(&m.body),escape_html(&m.created_at_display)).expect("mail message");
    }
    body.push_str("</div>");
    if page.has_more {
        if let Some(first) = page.items.first() {
            write!(
                body,
                "<a href=\"/mail/{id}?before_id={}\">Предыдущие сообщения</a>",
                first.id, first.id
            )
            .expect("mail cursor");
        }
    }
    if page.blocked {
        body.push_str("<p>Новые сообщения заблокированы.</p>");
    } else {
        write!(body,"<form class=\"comments-form\" action=\"/mail/{id}/messages\" method=\"post\"><label for=\"mail-body\">Ваше сообщение</label><textarea class=\"ui-textarea comments-form__input\" id=\"mail-body\" name=\"body\" rows=\"3\" required></textarea><input type=\"hidden\" name=\"command_key\" value=\"{}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--primary\" type=\"submit\">Отправить сообщение</button></form>",escape_html(key)).expect("mail form");
    }
    let (value, label) = if page.blocked_by_me {
        ("false", "Снять мою блокировку")
    } else {
        ("true", "Заблокировать собеседника")
    };
    write!(body,"<form action=\"/mail/{id}/block\" method=\"post\"><input type=\"hidden\" name=\"blocked\" value=\"{value}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">{label}</button></form></section><script defer src=\"/assets/mail.js\"></script>").expect("mail block");
    shell(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_08_plain_text_never_becomes_html_or_a_private_preview() {
        let page = MessagePage {
            items: vec![tor_social::domain::Message {
                id: 1,
                sender_id: 2,
                body: "<img onerror=alert(1)> /books/secret".into(),
                created_at: "2026-10-30T00:00:00Z".into(),
                created_at_display: "30.10.2026 03:00".into(),
            }],
            peer_id: 2,
            blocked: false,
            blocked_by_me: false,
            has_more: false,
        };
        let html = conversation(1, 1, &page, "Peer <safe>", "csrf", "key").0;
        assert!(html.contains("&lt;img onerror=alert(1)&gt;"));
        assert!(!html.contains("<img onerror"));
        assert!(!html.contains("href=\"/books/secret"));
    }
}
