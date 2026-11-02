use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_support::domain::{Chat, MessagePage};

pub(super) fn invalid_message(body: &str, csrf: &str, base: &str) -> Html<String> {
    layout("Не удалось отправить сообщение","Проверьте текст обращения.",&format!(
        "<section class=\"me-page\"><div class=\"container\"><h1>Не удалось отправить сообщение</h1><p role=\"alert\">Введите от 1 до 4000 символов. Текст не отправлен; его можно исправить ниже.</p><form class=\"auth-form\" method=\"post\" action=\"{base}/messages\"><label for=\"support-body\">Ваше сообщение</label><textarea class=\"ui-textarea\" id=\"support-body\" name=\"body\" rows=\"8\">{}</textarea><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><button class=\"ui-button ui-button--primary\">Отправить сообщение</button></form><p><a href=\"{base}\">Вернуться к переписке</a></p></div></section>",escape_html(body),escape_html(csrf)))
}

pub(super) fn chat(data: &MessagePage, csrf: &str, admin: bool) -> Html<String> {
    let base = if admin {
        format!("/admin/support/{}", data.chat.id)
    } else {
        "/support".into()
    };
    let mut body =
        String::from("<section class=\"me-page\"><div class=\"container\"><h1>Поддержка</h1>");
    if admin {
        body.push_str("<p><a href=\"/admin/support\">Все обращения</a></p>");
    } else if data.chat.requester_id.is_none() {
        write!(body,"<p>{}. Если войдёте, переписка сохранится. Без входа она удалится через 30 дней.</p><p><a href=\"/login\">Войти в учётную запись</a></p>",escape_html(&data.chat.guest_alias)).expect("support alias");
    } else {
        body.push_str("<p>Вы пишете из своей учётной записи.</p>");
    }
    if admin {
        write!(body,"<div id=\"support-messages\" data-admin-support-thread data-poll-url=\"{base}/messages\" aria-live=\"polite\">").expect("support admin thread");
    } else {
        body.push_str("<div id=\"support-messages\" aria-live=\"polite\">");
    }

    for m in &data.messages {
        let sender = if m.sender_kind == "admin" {
            "Поддержка"
        } else {
            "Обращение"
        };
        write!(body,"<article class=\"me-now-card\" data-support-message=\"{}\" data-message-id=\"{}\"><strong>{sender}</strong><p class=\"support-message-body\">{}</p><time>{}</time></article>",m.id,m.id,escape_html(&m.body),escape_html(&m.created_at)).expect("support message");
    }
    body.push_str("</div>");
    if admin {
        if let Some(last) = data.messages.last() {
            write!(body,"<form method=\"post\" action=\"{base}/read\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"message_id\" value=\"{}\"><button>Отметить обращение прочитанным</button></form>",escape_html(csrf),last.id).expect("support read form");
        }
    }

    if data.has_more {
        if let Some(m) = data.messages.first() {
            write!(
                body,
                "<p><a href=\"{base}?before_id={}\">Предыдущие сообщения</a></p>",
                m.id
            )
            .expect("support cursor");
        }
    }
    write!(body,"<form method=\"post\" action=\"{base}/messages\" class=\"auth-form\" data-admin-support-form><label for=\"support-body\">Ваше сообщение</label><textarea class=\"ui-textarea\" id=\"support-body\" name=\"body\" data-admin-support-input rows=\"4\" required></textarea><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"client_message_id\" data-admin-support-client-id value=\"{}\"><input type=\"hidden\" name=\"website\" value=\"\"><button class=\"ui-button ui-button--primary\" data-admin-support-submit>Отправить сообщение</button></form><p><a href=\"{base}\">Обновить переписку</a></p></div></section>",escape_html(csrf),tor_accounts::services::security::generate_session_token()).expect("support form");
    if admin {
        body.push_str("<p data-admin-support-error role=\"alert\" hidden></p><script src=\"/assets/newboo-admin-support.js\" defer></script>");
    }
    layout("Поддержка", "Переписка со службой поддержки.", &body)
}

pub(super) fn index(chats: &[(Chat, String)]) -> Html<String> {
    let mut body = String::from(
        "<section class=\"me-page\"><div class=\"container\"><h1>Обращения в поддержку</h1>",
    );
    if chats.is_empty() {
        body.push_str("<p>Обращений пока нет.</p>");
    }
    for (chat, name) in chats {
        let unread = if chat.unread {
            "Не прочитано"
        } else {
            "Прочитано"
        };
        write!(body,"<article class=\"me-now-card\"><h2><a href=\"/admin/support/{}\">{}</a></h2><p>{}</p><p>{unread}</p></article>",chat.id,escape_html(name),escape_html(&chat.last_message_preview)).expect("support list");
    }
    if chats.len() == 100 {
        if let Some((last, _)) = chats.last() {
            write!(
                body,
                "<a href=\"/admin/support?before={}\">Следующие обращения</a>",
                last.id
            )
            .expect("support next");
        }
    }
    body.push_str("</div></section>");
    layout("Поддержка", "Обращения пользователей.", &body)
}
