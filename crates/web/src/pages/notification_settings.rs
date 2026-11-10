//! Server-rendered controls for future external delivery cadence.
use super::{escape_html, layout};
use axum::response::Html;
use tor_reading::services::{
    notification_preferences::DeliveryMode, notification_send_time::PreferredSendHour,
};

pub fn delivery_preferences_page(
    mode: DeliveryMode,
    csrf: &str,
    smart_hour: Option<PreferredSendHour>,
    email_opted_in: bool,
    proof_mail_enabled: bool,
) -> Html<String> {
    let checked = |choice: DeliveryMode| if choice == mode { " checked" } else { "" };
    let smart_estimate = if mode != DeliveryMode::SmartTime {
        String::new()
    } else if let Some(hour) = smart_hour.filter(|hour| hour.is_reliable()) {
        format!(
            "<p class=\"auth-form__help\">По подтверждённым первым чтениям предварительная оценка: {:02}:00 UTC ({} событий). Это не расписание активной отправки.</p>",
            hour.hour_utc, hour.observed_starts
        )
    } else {
        "<p class=\"auth-form__help\">Пока недостаточно подтверждённых начал чтения для выбора часа. Отправка не настроена.</p>".to_owned()
    };
    let email_controls = if email_opted_in {
        format!(
            r#"<p class="auth-form__help">Адрес email подтверждён для уведомлений о главах. Отправка пока не подключена.</p>
<form method="post" action="/me/notification-email/revoke" class="auth-form">
<input type="hidden" name="csrf_token" value="{}">
<button class="button" type="submit">Отозвать согласие на email</button>
</form>"#,
            escape_html(csrf)
        )
    } else if proof_mail_enabled {
        format!(
            r#"<p class="auth-form__help">Для получения уведомлений отдельно подтвердите адрес email вашей учётной записи. Никакие уведомления о книгах до включения отправителя не посылаются.</p>
<form method="post" action="/me/notification-email/request" class="auth-form">
<input type="hidden" name="csrf_token" value="{0}">
<button class="button" type="submit">Отправить код подтверждения email</button>
</form>
<form method="post" action="/me/notification-email/confirm" class="auth-form">
<input type="hidden" name="csrf_token" value="{0}">
<label>Код из письма <input type="text" name="proof_code" minlength="43" maxlength="43" required autocomplete="off" spellcheck="false"></label>
<label><input type="checkbox" name="email_opt_in" value="on" required> Я согласен получать email-уведомления о новых главах.</label>
<button class="button" type="submit">Подтвердить email и согласие</button>
</form>"#,
            escape_html(csrf)
        )
    } else {
        "<p class=\"auth-form__help\">Подтверждение email пока не подключено. Существующий адрес профиля не является согласием на рассылку.</p>".to_owned()
    };
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
<label><input type="radio" name="delivery_mode" value="smart_time"{smart}> В удобное время</label>
<label><input type="radio" name="delivery_mode" value="daily_digest"{daily}> Ежедневная сводка</label>
<label><input type="radio" name="delivery_mode" value="disabled"{disabled}> Отключено</label>
</fieldset>
{smart_estimate}
<p class="auth-form__help">Внешняя рассылка пока не подключена: уведомления о книгах не отправляются. Выбор сохраняется для будущей доставки и не заменяет разрешение на конкретный канал.</p>
<button class="button" type="submit">Сохранить настройки</button>
</form>
{email_controls}
<p><a href="/me">Вернуться в кабинет</a></p>
</div>
</div>
</section>"#,
        csrf = escape_html(csrf),
        smart_estimate = smart_estimate,
        email_controls = email_controls,
        immediate = checked(DeliveryMode::Immediate),
        smart = checked(DeliveryMode::SmartTime),
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
        let html = delivery_preferences_page(DeliveryMode::DailyDigest, "<bad>", None, false, false).0;
        assert!(html.contains("value=\"daily_digest\" checked"));
        assert!(!html.contains("value=\"immediate\" checked"));
        assert!(!html.contains("value=\"smart_time\" checked"));
        assert!(!html.contains("value=\"disabled\" checked"));
        assert!(html.contains("value=\"&lt;bad&gt;\""));
        assert!(!html.contains("<bad>"));
        assert!(html.contains("пока не подключена"));
        assert!(html.contains("/me/library#new-chapters"));
        assert!(!html.contains("<script"));
        let smart = delivery_preferences_page(DeliveryMode::SmartTime, "csrf", None, false, false).0;
        assert!(smart.contains("value=\"smart_time\" checked"));
        assert!(!smart.contains("value=\"daily_digest\" checked"));
        assert!(smart.contains("недостаточно подтверждённых"));
        let mut buckets = [0; 24];
        buckets[18] = 8;
        let reliable =
            tor_reading::services::notification_send_time::choose_preferred_hour(&buckets);
        let with_hour =
            delivery_preferences_page(DeliveryMode::SmartTime, "csrf", Some(reliable), false, false).0;
        assert!(with_hour.contains("18:00 UTC"));
        assert!(with_hour.contains("не расписание активной отправки"));
        let disabled = delivery_preferences_page(DeliveryMode::Disabled, "csrf", None, false, false).0;
        assert!(disabled.contains("value=\"disabled\" checked"));
        let opt_in = delivery_preferences_page(DeliveryMode::Immediate, "csrf", None, true, false).0;
        assert!(opt_in.contains("Отозвать согласие"));
        let proof = delivery_preferences_page(DeliveryMode::Disabled, "csrf", None, false, true).0;
        assert!(proof.contains("/me/notification-email/confirm"));
        assert!(proof.contains("name=\"email_opt_in\" value=\"on\" required"));
        assert!(!proof.contains("<script"));
    }
}
