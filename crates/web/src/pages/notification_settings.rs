//! Server-rendered controls for future external delivery cadence.
use super::{escape_html, layout};
use axum::response::Html;
use tor_reading::repositories::DeliveryMode;

pub fn delivery_preferences_page(mode: DeliveryMode, csrf: &str) -> Html<String> {
    let checked = |choice: DeliveryMode| if choice == mode { " checked" } else { "" };
    let content = format!(
        r#"<section class="section auth-section">
<div class="container">
<div class="auth-card author-card">
<p class="eyebrow">Личный кабинет</p>
<h1>Уведомления о новых главах</h1>
<p>Ваша лента бесплатных и платных глав доступна в <a href="/me/library#new-chapters">библиотеке</a> независимо от этих настроек.</p>
<form method="post" action="/me/notification-settings" class="auth-form">
<input type="hidden" name="csrf_token" value="{csrf}">
<fieldset class="notification-delivery-choices">
<legend>Будущая внешняя доставка</legend>
<label><input type="radio" name="delivery_mode" value="immediate"{immediate}> Сразу после выхода главы</label>
<label><input type="radio" name="delivery_mode" value="daily_digest"{daily}> Ежедневная сводка</label>
<label><input type="radio" name="delivery_mode" value="disabled"{disabled}> Отключено</label>
</fieldset>
<p class="auth-form__help">Внешняя рассылка пока не подключена. Сейчас никакие письма или сообщения не отправляются. Выбор только сохраняется для будущей доставки и не заменяет разрешение на конкретный канал.</p>
<button class="button" type="submit">Сохранить настройки</button>
</form>
<p><a href="/me">Вернуться в кабинет</a></p>
</div>
</div>
</section>"#,
        csrf = escape_html(csrf),
        immediate = checked(DeliveryMode::Immediate),
        daily = checked(DeliveryMode::DailyDigest),
        disabled = checked(DeliveryMode::Disabled),
    );
    layout(
        "Настройки уведомлений",
        "Периодичность будущих уведомлений о новых главах.",
        &content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_17_preferences_form_is_safe_and_does_not_claim_delivery() {
        let html = delivery_preferences_page(DeliveryMode::DailyDigest, "<bad>").0;
        assert!(html.contains("value=\"daily_digest\" checked"));
        assert!(!html.contains("value=\"immediate\" checked"));
        assert!(!html.contains("value=\"disabled\" checked"));
        assert!(html.contains("value=\"&lt;bad&gt;\""));
        assert!(!html.contains("<bad>"));
        assert!(html.contains("пока не подключена"));
        assert!(html.contains("/me/library#new-chapters"));
        assert!(!html.contains("<script"));
        let disabled = delivery_preferences_page(DeliveryMode::Disabled, "csrf").0;
        assert!(disabled.contains("value=\"disabled\" checked"));
    }
}
