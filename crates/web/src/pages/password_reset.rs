use super::{escape_html, layout};
use axum::response::Html;

pub fn reset_request_form(csrf: &str, enabled: bool, message: Option<&str>) -> Html<String> {
    let message = message.unwrap_or(if enabled {
        "Укажите почту вашей учётной записи."
    } else {
        "Восстановление по почте временно недоступно."
    });
    let body = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card">
<h1>Забыли пароль?</h1><p role="status">{message}</p>
<form class="auth-form" method="post" action="/forgot-password">
<input type="hidden" name="csrf_token" value="{csrf}">
<label for="reset-email">Электронная почта</label><input id="reset-email" name="email" type="email" autocomplete="email" maxlength="254" required>
<button class="button" type="submit"{disabled}>Отправить ссылку</button></form>
<p><a href="/login">Вернуться ко входу</a></p></div></div></section>"#,
        csrf = escape_html(csrf),
        message = escape_html(message),
        disabled = if enabled { "" } else { " disabled" }
    );
    layout("Восстановление доступа", "Смена забытого пароля.", &body)
}

pub fn reset_confirm_form(csrf: &str, message: Option<&str>) -> Html<String> {
    let banner = message
        .map(|m| format!("<p role=\"alert\">{}</p>", escape_html(m)))
        .unwrap_or_default();
    let body = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card">
<h1>Новый пароль</h1>{banner}
<form class="auth-form" method="post" action="/reset-password" novalidate>
<input type="hidden" name="csrf_token" value="{csrf}">
<label for="reset-token">Код из письма</label><input id="reset-token" name="token" autocomplete="off" minlength="43" maxlength="43" required>
<label for="reset-password">Новый пароль</label><input id="reset-password" name="new_password" type="password" autocomplete="new-password" minlength="12" maxlength="512" required>
<label for="reset-confirm">Повторите новый пароль</label><input id="reset-confirm" name="confirm_password" type="password" autocomplete="new-password" maxlength="512" required>
<button class="button" type="submit">Сохранить пароль</button></form>
<p><a href="/forgot-password">Запросить новую ссылку</a></p></div></div></section>
<script src="/assets/account-reset.js" defer></script>"#,
        csrf = escape_html(csrf)
    );
    layout("Новый пароль", "Подтверждение смены пароля.", &body)
}
