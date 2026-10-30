use super::{escape_html, layout};
use axum::response::Html;
use tor_accounts::repositories::profile::Profile;

pub fn profile_settings(p: &Profile, csrf: &str, error: Option<&str>) -> Html<String> {
    let options = |enabled: bool| {
        format!(
            "<option value=\"false\"{}>Нет</option><option value=\"true\"{}>Да</option>",
            if enabled { "" } else { " selected" },
            if enabled { " selected" } else { "" }
        )
    };
    let banner = error
        .map(|m| format!("<p role=\"alert\">{}</p>", escape_html(m)))
        .unwrap_or_default();
    let body = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-card">
<h1>Настройки профиля</h1><p>Эти настройки доступны только вам.</p>{banner}
<form class="auth-form" action="/me/settings" method="post">
<input type="hidden" name="csrf_token" value="{csrf}">
<input type="hidden" name="expected_revision" value="{revision}">
<label for="profile-name">Имя</label><input id="profile-name" name="display_name" maxlength="80" value="{name}" required>
<label for="profile-handle">Адрес профиля</label><input id="profile-handle" name="handle" maxlength="32" value="{handle}">
<p>От 3 до 32 латинских букв, цифр, точек или подчёркиваний. Пустое поле удаляет адрес.</p>
<label for="profile-adult">Мне исполнилось 18 лет</label><select id="profile-adult" name="adult_enabled">{adult}</select>
<button class="button" type="submit">Сохранить настройки</button></form>
<p><a href="/me/notification-settings">Уведомления</a> · <a href="/me/settings/password">Сменить пароль</a> · <a href="/me">Личный кабинет</a></p>
</div></div></section>"#,
        csrf = escape_html(csrf),
        revision = p.revision,
        name = escape_html(&p.display_name),
        handle = escape_html(p.handle.as_deref().unwrap_or("")),
        adult = options(p.adult_enabled),
    );
    layout(
        "Настройки профиля",
        "Личные настройки учётной записи.",
        &body,
    )
}
