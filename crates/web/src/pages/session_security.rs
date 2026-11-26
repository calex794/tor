//! Authenticated device/session list. Never render passwords, tokens or raw IP.
use super::{escape_html, private_account_layout as layout};
use axum::response::Html;
use std::fmt::Write;
use tor_accounts::repositories::{security_history::SecurityEvent, sessions::ActiveSession};

fn browser_label(family: &str) -> &'static str {
    match family {
        "chrome" => "Chrome",
        "firefox" => "Firefox",
        "edge" => "Edge",
        "safari" => "Safari",
        "other" => "Другой браузер",
        _ => "Неизвестный браузер",
    }
}

fn security_history(history: Option<&[SecurityEvent]>) -> String {
    let Some(events) = history else {
        return "<li>История временно недоступна. Управление сеансами работает отдельно.</li>"
            .to_owned();
    };
    if events.is_empty() {
        return "<li>За последние 90 дней событий не найдено.</li>".to_owned();
    }
    let mut rows = String::new();
    for event in events.iter().take(50) {
        let label = match event.event_kind.as_str() {
            "registration" => "Регистрация аккаунта",
            "password_login" => "Вход в аккаунт",
            "password_change" => "Смена пароля",
            "password_reset" => "Сброс пароля",
            "session_ended" => "Завершение сеанса",
            "owner_revoke" => "Отзыв сеанса",
            "owner_revoke_others" => "Отзыв остальных сеансов",
            _ => "Событие безопасности",
        };
        let _ = write!(
            rows,
            "<li class=\"me-now-item\"><div><strong>{label}</strong><p>{} UTC</p>",
            escape_html(&event.occurred_utc),
        );
        if let Some(browser) = event.browser_family.as_deref() {
            let _ = write!(rows, "<p>Браузер: {}</p>", browser_label(browser));
        }
        if let Some(count) = event.revoked_count {
            let _ = write!(rows, "<p>Завершено сеансов: {count}</p>");
        }
        rows.push_str("</div></li>");
    }
    rows
}

pub fn active_sessions(
    csrf: &str,
    sessions: &[ActiveSession],
    history: Option<&[SecurityEvent]>,
) -> Html<String> {
    let mut rows = String::new();
    for session in sessions {
        let agent = browser_label(&session.browser_family);
        let current_label = if session.current {
            " — это устройство"
        } else {
            ""
        };
        let _ = write!(
            rows,
            "<li class=\"me-now-item\"><div><strong>{agent}{current_label}</strong>\
             <p>Вход (UTC): {opened}</p></div>\
             <form method=\"post\" action=\"/me/security/sessions\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"session_id\" value=\"{id}\">\
             <label>Текущий пароль<input type=\"password\" name=\"current_password\" autocomplete=\"current-password\" maxlength=\"512\" required></label>\
             <button class=\"button button--small\" type=\"submit\">Завершить сеанс</button>\
             </form></li>",
            opened = escape_html(&session.opened_utc),
            csrf = escape_html(csrf),
            id = escape_html(&session.id)
        );
    }
    if sessions.is_empty() {
        rows.push_str("<li>Действующих сеансов не найдено.</li>");
    }
    let body = include_str!("../../templates/security_sessions.html")
        .replace("<!--SESSIONS-->", &rows)
        .replace("<!--SECURITY_HISTORY-->", &security_history(history));
    layout(
        "Активные сеансы",
        "Завершение входа на других устройствах.",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_distinguishes_unavailable_empty_and_success() {
        assert!(security_history(None).contains("временно недоступна"));
        assert!(security_history(Some(&[])).contains("событий не найдено"));
        let events = [SecurityEvent {
            event_kind: "<script>untrusted</script>".to_owned(),
            occurred_utc: "<img src=x>".to_owned(),
            browser_family: Some("untrusted-browser".to_owned()),
            revoked_count: Some(2),
        }];
        let html = security_history(Some(&events));
        assert!(html.contains("Событие безопасности"));
        assert!(html.contains("&lt;img src=x&gt;"));
        assert!(html.contains("Неизвестный браузер"));
        assert!(html.contains("Завершено сеансов: 2"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("untrusted-browser"));
    }
}
