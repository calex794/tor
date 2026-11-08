//! Browser-held, no-store challenge UI. No reset credentials in Telegram.
use super::{escape_html, layout};
use axum::response::Html;

fn page(body: &str) -> Html<String> {
    layout("Восстановление через Telegram", "Смена пароля с подтверждением через Telegram.", body)
}

pub fn begin_form(csrf: &str) -> Html<String> {
    page(&form_markup(csrf))
}

fn form_markup(csrf: &str) -> String {
    format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card">
<h1>Восстановление через Telegram</h1>
<p>Если к учётной записи заранее привязан Telegram, бот получит запрос на подтверждение. 
Наличие аккаунта и Telegram не раскрывается.</p>
<form method="post" action="/recover/telegram" class="auth-form">
<input type="hidden" name="csrf_token" value="{csrf}">
<label for="tg-recover-email">Почта учётной записи TOR</label>
<input id="tg-recover-email" name="email" type="email" autocomplete="email" maxlength="254" required>
<button class="button" type="submit">Отправить запрос в Telegram</button>
</form>
<p><a href="/recover/backup">У меня есть резервный код</a> · <a href="/login">Вернуться ко входу</a></p>
</div></div></section>"#,
        csrf = escape_html(csrf),
    )
}

pub fn challenge(csrf: &str, match_code: &str, status: &str) -> Html<String> {
    let step = match status {
        "approved" => format!(
            "<p>Подтверждение в Telegram получено. Эта заявка действует только в начавшем её браузере.</p>\
             <form method=\"post\" action=\"/recover/telegram/complete\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
             <button class=\"button\" type=\"submit\">Продолжить смену пароля</button>\
             </form>",
            escape_html(csrf)
        ),
        "denied" | "used" => "<p>Заявка отклонена либо уже использована. При необходимости создайте новую.</p>".to_owned(),
        _ => "<p>Проверьте сообщение бота в своём Telegram. 
              Сверьте оба кода и нажмите «Подтвердить» либо «Отклонить» в боте. 
              Если сообщения нет, заявка может быть недоступна или завершена.</p>
              <p>После подтверждения обновите эту страницу.</p>".to_owned(),
    };
    let body = format!(
        "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card\">\
         <h1>Подтверждение смены пароля</h1>\
         <p>Код сверки: <strong>{}</strong> (не пароль и не код доступа)</p>\
         {}<p><a href=\"/recover/telegram\">Обновить статус</a></p>\
         </div></div></section>{}",
        escape_html(match_code), step, form_markup(csrf)
    );
    page(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_07_browser_recovery_shows_only_nonsecret_match_code_and_csrf() {
        let shown = challenge("csrf-value", "abc_12", "pending").0;
        assert!(shown.contains("abc_12"));
        assert!(shown.contains("name=\"csrf_token\""));
        assert!(!shown.contains("reset-password#"));
        assert!(!shown.contains("callback_data"));
        let approved = challenge("csrf-value", "xyz_90", "approved").0;
        assert!(approved.contains("/recover/telegram/complete"));
        assert!(!shown.contains("/recover/telegram/complete"));
    }
}
