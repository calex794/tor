//! Authenticated device/session list. Never render passwords, tokens or raw IP.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_accounts::repositories::sessions::ActiveSession;

pub fn active_sessions(csrf: &str, sessions: &[ActiveSession]) -> Html<String> {
    let mut rows = String::new();
    for session in sessions {
        let agent = match session.browser_family.as_str() {
            "chrome" => "Chrome",
            "firefox" => "Firefox",
            "edge" => "Edge",
            "safari" => "Safari",
            "other" => "Другой браузер",
            _ => "Неизвестный браузер",
        };
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
    let body =
        include_str!("../../templates/security_sessions.html").replace("<!--SESSIONS-->", &rows);
    layout(
        "Активные сеансы",
        "Завершение входа на других устройствах.",
        &body,
    )
}
