//! Render only aggregate local estimates and numeric operator controls.
use super::{pages, CostAdminView};

pub(super) fn money(value: i64) -> String {
    format!("{}.{:02} ₽", value / 100, value % 100)
}

pub(super) fn body(view: &CostAdminView, csrf: &str) -> String {
    let snapshot = &view.snapshot;
    let l = snapshot.limits;
    let enabled = if l.enabled {
        "Бюджетный допуск разрешён"
    } else {
        "Отправка остановлена"
    };
    let usage = match snapshot
        .usage
        .as_ref()
        .and_then(|u| u.estimate_kopecks(0).map(|cost| (u, cost)))
    {
        Some((u, cost)) => {
            let warning = if cost >= l.monthly_limit {
                "Достигнут предел бюджета"
            } else if cost >= l.news_pause {
                "Новости приостановлены"
            } else if cost >= l.warning_2 {
                "Второй порог расходов"
            } else if cost >= l.warning_1 {
                "Первый порог расходов"
            } else {
                "В пределах бюджета"
            };
            let free_status = if u.free_allowance == 0 {
                "Бесплатная квота не подтверждена или недоступна."
            } else if u.reserved_attempts >= u.free_allowance {
                "Учтённая бесплатная квота исчерпана. Регистрация не блокируется по этому порогу."
            } else {
                "Бесплатная квота ещё доступна."
            };
            format!(
                "<dl><dt>Расчётный месяц (МСК)</dt><dd>{}</dd>\
                 <dt>Оценка расходов</dt><dd>{}</dd>\
                 <dt>Остаток бюджета</dt><dd>{}</dd>\
                 <dt>Зарезервировано попыток</dt><dd>{}</dd>\
                 <dt>Учтённая бесплатная квота</dt><dd>{}</dd>\
                 <dt>Остаток бесплатной квоты</dt><dd>{}</dd></dl>\
                 <p><strong>{warning}</strong></p>\
                 <p>{free_status}</p>",
                pages::escape_html(&u.month_start),
                money(cost),
                money(l.monthly_limit.saturating_sub(cost).max(0)),
                u.reserved_attempts,
                u.free_allowance,
                u.free_allowance.saturating_sub(u.reserved_attempts).max(0),
            )
        }
        None => "<p>Месяц не сверён с Postbox. Отправка запрещена.</p>".to_owned(),
    };
    let boundary = if snapshot.boundary_pause {
        "<p>Пауза на границе расчётного месяца.</p>"
    } else {
        ""
    };
    let csrf = pages::escape_html(csrf);
    let reconciliation = reconciliation_form(view, &csrf);
    format!(
        "<section class=\"section\"><div class=\"container\">\
         <h1>Системная почта</h1><p><a href=\"/odmen\">Администрирование</a></p>\
         <p><strong>{enabled}</strong></p>{usage}{boundary}\
         <p>Это консервативная локальная оценка, не счёт Яндекс Облака. \
         Резервации учитываются даже при неопределённом результате. \
         Бесплатная квота не является лимитом регистраций.</p>\
         <h2>Денежные пороги (рубли за месяц)</h2>\
         <form method=\"post\" action=\"/odmen/system-mail\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
         <input type=\"hidden\" name=\"revision\" value=\"{}\">\
         <label>Первое предупреждение <input type=\"number\" name=\"warning_1\" min=\"1\" max=\"100000\" value=\"{}\" required></label>\
         <label>Второе предупреждение <input type=\"number\" name=\"warning_2\" min=\"1\" max=\"100000\" value=\"{}\" required></label>\
         <label>Пауза новостей <input type=\"number\" name=\"news_pause\" min=\"1\" max=\"100000\" value=\"{}\" required></label>\
         <label>Бюджет Postbox <input type=\"number\" name=\"monthly_limit\" min=\"1\" max=\"100000\" value=\"{}\" required></label>\
         <button type=\"submit\">Сохранить пороги</button></form>\
         <form method=\"post\" action=\"/odmen/system-mail/stop\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
         <button type=\"submit\">Остановить отправку</button></form>\
         <p>Остановка запрещает новые допуски; уже начатая отправка может завершиться. \
         Включение почты требует отдельного подтверждения. \
         Пороговые предупреждения отображаются здесь, внешняя доставка уведомлений не подключена.</p>\
         {reconciliation}</div></section>",
        view.revision,
        l.warning_1 / 100,
        l.warning_2 / 100,
        l.news_pause / 100,
        l.monthly_limit / 100,
    )
}

fn reconciliation_form(view: &CostAdminView, escaped_csrf: &str) -> String {
    if view.snapshot.limits.enabled || view.snapshot.boundary_pause {
        return "<p>Для сверки месяца остановите отправку и дождитесь окончания переходного периода.</p>"
            .to_owned();
    }
    let usage = view.snapshot.usage.as_ref();
    let month = usage
        .and_then(|u| u.month_start.get(..7))
        .map(pages::escape_html)
        .unwrap_or_default();
    let free = usage.map_or(0, |u| u.free_allowance);
    let baseline = usage
        .map(|u| u.baseline_kopecks.to_string())
        .unwrap_or_default();
    let rate = usage
        .map(|u| u.rate_per_thousand.to_string())
        .unwrap_or_default();
    let attempts = usage
        .map(|u| u.reserved_attempts.to_string())
        .unwrap_or_default();
    format!(
        "<h2>Сверка расчётного месяца</h2>\
         <p>Сначала независимо проверьте данные биллинга Postbox и историю резервирований. \
         Если бесплатная квота неизвестна, укажите 0. Не уменьшайте учтённые расходы. \
         SHA-256 связывает запись с проверенным документом, но не подтверждает его достоверность. \
         Сверка не включает отправку и не заменяет проверку после восстановления.</p>\
         <form method=\"post\" action=\"/odmen/system-mail/reconcile\" autocomplete=\"off\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{escaped_csrf}\">\
         <input type=\"hidden\" name=\"revision\" value=\"{}\">\
         <label>Месяц (МСК) <input type=\"month\" name=\"month\" value=\"{month}\" required></label>\
         <label>Подтверждённая бесплатная квота <input type=\"number\" name=\"free_allowance\" min=\"0\" max=\"2000\" value=\"{free}\" required></label>\
         <label>Учтённые внешние расходы (копейки) <input type=\"number\" name=\"baseline_kopecks\" min=\"0\" value=\"{baseline}\" required></label>\
         <label>Верхний тариф за 1000 писем (копейки) <input type=\"number\" name=\"rate_per_thousand\" min=\"8032\" value=\"{rate}\" required></label>\
         <label>Накопленные попытки <input type=\"number\" name=\"reserved_attempts\" min=\"0\" value=\"{attempts}\" required></label>\
         <label>SHA-256 проверенных материалов <input name=\"evidence_sha256\" minlength=\"64\" maxlength=\"64\" pattern=\"[0-9a-fA-F]{{64}}\" required></label>\
         <label><input type=\"checkbox\" name=\"reviewed\" value=\"yes\" required> Я проверил расходы, тариф и восстановленную историю независимо от этой базы.</label>\
         <button type=\"submit\">Сохранить сверку без включения отправки</button></form>",
        view.revision,
    )
}

#[cfg(test)]
mod tests;
