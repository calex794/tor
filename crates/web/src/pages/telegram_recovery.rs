//! A/S004: server-rendered, no-store Telegram recovery pages.
use super::{escape_html, layout};
use axum::response::Html;

fn auth_page(title: &str, content: &str) -> Html<String> {
    layout(title, "Безопасность учётной записи TOR.", content)
}

pub fn settings_link(html: Html<String>) -> Html<String> {
    Html(html.0.replace(
        "Сменить пароль</a> · ",
        "Сменить пароль</a> · <a href=\"/me/security/telegram\">Telegram и резервные коды</a> · ",
    ))
}

pub fn link_page(
    csrf: &str,
    linked: Option<i64>,
    candidate: Option<i64>,
    deep_link: Option<&str>,
    notice: Option<&str>,
) -> Html<String> {
    let status = match linked {
        Some(id) => format!("<p>Привязан Telegram ID: {}.</p>", id),
        None => "<p>Telegram ещё не привязан.</p>".to_owned(),
    };
    let unlink_form = if linked.is_some() {
        format!(
            "<section><h2>Отвязать Telegram</h2>\
             <p>Для удаления привязки нужен текущий пароль. При отвязке \
             все резервные коды и ещё не использованные токены Telegram \
             аннулируются. Убедитесь, что у вас остаётся доступ к аккаунту.</p>\
             <form method=\"post\" action=\"/me/security/telegram/unlink\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
             <label for=\"tg-unlink-password\">Текущий пароль для отвязки</label>\
             <input id=\"tg-unlink-password\" type=\"password\" name=\"current_password\" \
             autocomplete=\"current-password\" maxlength=\"512\" required>\
             <button class=\"button\" type=\"submit\">Отвязать Telegram и удалить резервные коды</button>\
             </form></section>",
            escape_html(csrf)
        )
    } else {
        String::new()
    };

    let pending = candidate.map_or_else(String::new, |id| {
        format!(
            "<p><strong>Заявка из Telegram ID {id}.</strong> Сверьте этот ID с сообщением вашего бота. Если вы не открывали ссылку, ничего не подтверждайте.</p>\
             <form method=\"post\" action=\"/me/security/telegram/confirm\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
             <button class=\"button\" type=\"submit\">Подтвердить этот Telegram</button></form>",
            csrf = escape_html(csrf),
        )
    });
    let link = deep_link.map_or_else(String::new, |url| {
        format!(
            "<p>Откройте ссылку и запустите бота; она действует 5 минут:</p>\
             <p><a rel=\"noreferrer noopener\" href=\"{}\">Открыть Telegram-бота</a></p>\
             <p>Затем вернитесь сюда, обновите страницу и подтвердите Telegram ID.</p>",
            escape_html(url),
        )
    });
    let message = notice.map_or_else(String::new, |msg| {
        format!("<p role=\"status\">{}</p>", escape_html(msg))
    });
    let body = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card">
<h1>Telegram и восстановление доступа</h1>
<p>Привязка требует текущего пароля. Восстановление из бота недоступно администраторам и аккаунтам с правами авторов/модераторов.</p>
{status}{message}{pending}{link}{unlink_form}
<form method="post" action="/me/security/telegram/begin">
<input type="hidden" name="csrf_token" value="{csrf}">
<label for="tg-current-password">Текущий пароль</label>
<input id="tg-current-password" type="password" name="current_password" maxlength="512" autocomplete="current-password" required>
<button class="button" type="submit">Начать новую привязку</button></form>
<p><a href="/me/settings">Вернуться к настройкам</a></p>
</div></div></section>"#,
        csrf = escape_html(csrf),
        unlink_form = unlink_form,
    );
    auth_page("Telegram — безопасность", &body)
}

pub fn show_backup_codes(codes: &[String; 8]) -> Html<String> {
    let rows = codes
        .iter()
        .map(|s| format!("<li><code>{}</code></li>", escape_html(s)))
        .collect::<String>();
    let body = format!(
        "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card\">\
         <h1>Telegram привязан</h1><p><strong>Сохраните восемь резервных кодов сейчас.</strong>\
         Они показываются только один раз, действуют по одному и каждый позволяет восстановить доступ без Telegram.\
         Не храните их в Telegram.</p><ol>{rows}</ol>\
         <p><a href=\"/me/security/telegram\">Вернуться к безопасности</a></p>\
         </div></div></section>"
    );
    auth_page("Резервные коды", &body)
}

pub fn backup_form(csrf: &str, notice: Option<&str>) -> Html<String> {
    let message = notice.map_or_else(String::new, |m| {
        format!("<p role=\"status\">{}</p>", escape_html(m))
    });
    let body = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card">
<h1>Восстановление резервным кодом</h1>{message}
<p>Укажите адрес учётной записи и один заранее сохранённый резервный код. Код будет погашен при выдаче ссылки.</p>
<form method="post" action="/recover/backup">
<input type="hidden" name="csrf_token" value="{csrf}">
<label for="backup-email">Электронная почта</label>
<input type="email" id="backup-email" name="email" autocomplete="email" maxlength="254" required>
<label for="backup-code">Резервный код</label>
<input id="backup-code" name="backup_code" autocomplete="off" minlength="43" maxlength="43" required>
<button class="button" type="submit">Продолжить</button></form>
<p><a href="/forgot-password">Другие способы восстановления</a></p>
</div></div></section>"#,
        csrf = escape_html(csrf)
    );
    auth_page("Резервный код", &body)
}

pub fn backup_ready(token: &str) -> Html<String> {
    let body = format!(
        "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card\">\
         <h1>Код подтверждён</h1>\
         <p>Откройте страницу смены пароля. Ссылка действует 10 минут и только один раз.</p>\
         <p><a rel=\"noreferrer\" href=\"/reset-password#{}\">Установить новый пароль</a></p>\
         </div></div></section>",
        escape_html(token)
    );
    auth_page("Восстановление доступа", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_07_telegram_settings_never_embeds_password_or_tokens() {
        let page = link_page("csrf", None, Some(111), None, None).0;
        assert!(page.contains("Telegram ID 111"));
        assert!(page.contains("csrf_token"));
        assert!(!page.contains("password_hash"));
        assert!(!page.contains("recover#"));
        let link = link_page(
            "csrf",
            None,
            None,
            Some("https://t.me/TorBot?start=bind_test"),
            None,
        )
        .0;
        assert!(link.contains("rel=\"noreferrer noopener\""));
    }
}
