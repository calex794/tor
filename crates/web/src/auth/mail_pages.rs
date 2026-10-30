use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_social::domain::{Conversation, MessagePage};

fn shell(body: &str) -> Html<String> {
    layout("Личная почта","Личные диалоги и сообщения.",&format!("<section class=\"me-page mail-page\"><div class=\"container\"><nav class=\"dashboard-nav\" aria-label=\"Разделы кабинета\"><a class=\"dashboard-nav__item\" href=\"/me\">Обзор</a><a class=\"dashboard-nav__item is-active\" href=\"/mail\">Личная почта</a><a class=\"dashboard-nav__item\" href=\"/me/inbox\">Обсуждения</a></nav>{body}</div></section>"))
}

pub(super) fn index(actor: i64, rows: &[Conversation], limit: u16, csrf: &str) -> Html<String> {
    let csrf = escape_html(csrf);
    let mut body=format!("<header class=\"ui-page-header\"><div><h1 class=\"ui-page-header__title\">Личная почта</h1><p>Ваш номер для переписки: <strong data-mail-user=\"{actor}\">{actor}</strong>. Передайте его собеседнику.</p></div></header><section class=\"me-section\"><form action=\"/mail/start\" method=\"post\" class=\"comments-form\"><label for=\"mail-peer\">Номер получателя</label><input id=\"mail-peer\" name=\"peer_id\" type=\"number\" min=\"1\" required><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--primary\" type=\"submit\">Начать диалог</button></form></section><section class=\"me-section\" aria-label=\"Диалоги\">");
    if rows.is_empty() {
        body.push_str("<p>Диалогов пока нет.</p>");
    }
    for c in rows {
        let blocked = if c.blocked {
            " · Новые сообщения заблокированы"
        } else {
            ""
        };
        write!(body,"<article class=\"me-now-card\"><a href=\"/mail/{}\">Пользователь №{}</a><p>Непрочитанных: <span data-mail-unread=\"{}\">{}</span>{blocked}</p></article>",c.id,c.peer_id,c.id,c.unread).expect("mail markup");
    }
    if rows.len() == usize::from(limit) {
        if let Some(last) = rows.last() {
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
    csrf: &str,
    key: &str,
) -> Html<String> {
    let csrf = escape_html(csrf);
    let last = page.items.last().map_or(0, |m| m.id);
    let mut body=format!("<header class=\"ui-page-header\"><div><h1 class=\"ui-page-header__title\">Диалог с пользователем №{}</h1><p><a href=\"/mail\">Все диалоги</a></p></div></header><section class=\"me-section\"><p id=\"mail-status\" role=\"status\"></p><div id=\"mail-messages\" data-conversation=\"{id}\" data-last-id=\"{last}\" aria-live=\"polite\">",page.peer_id);
    for m in &page.items {
        let sender = if m.sender_id == actor {
            "Вы"
        } else {
            "Собеседник"
        };
        write!(body,"<article class=\"me-now-card\" data-message-id=\"{}\"><strong>{sender}</strong><p class=\"mail-message-body\">{}</p><time>{}</time></article>",m.id,escape_html(&m.body),escape_html(&m.created_at)).expect("mail message");
    }
    body.push_str("</div>");
    if page.has_more {
        if let Some(first) = page.items.first() {
            write!(
                body,
                "<a href=\"/mail/{id}?before_id={}\">Предыдущие сообщения</a>",
                first.id
            )
            .expect("mail cursor");
        }
    }
    {
        let hidden = if last == 0 { " hidden" } else { "" };
        write!(body,"<form action=\"/mail/{id}/read\" method=\"post\"{hidden}><input type=\"hidden\" name=\"through_id\" value=\"{last}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">Отметить показанные сообщения прочитанными</button></form>").expect("mail read");
    }
    if page.blocked {
        body.push_str("<p>Новые сообщения заблокированы.</p>");
    } else {
        write!(body,"<form class=\"comments-form\" action=\"/mail/{id}/messages\" method=\"post\"><label for=\"mail-body\">Ваше сообщение</label><textarea class=\"ui-textarea comments-form__input\" id=\"mail-body\" name=\"body\" rows=\"4\" maxlength=\"2000\" required></textarea><input type=\"hidden\" name=\"command_key\" value=\"{}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--primary\" type=\"submit\">Отправить сообщение</button></form>",escape_html(key)).expect("mail form");
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
            }],
            peer_id: 2,
            blocked: false,
            blocked_by_me: false,
            has_more: false,
        };
        let html = conversation(1, 1, &page, "csrf", "key").0;
        assert!(html.contains("&lt;img onerror=alert(1)&gt;"));
        assert!(!html.contains("<img onerror"));
        assert!(!html.contains("href=\"/books/secret"));
    }
}
