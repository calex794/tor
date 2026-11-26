//! Render only aggregate local estimates and numeric operator controls.
use super::{pages, CostAdminView};

fn money(value: i64) -> String {
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
            format!(
                "<dl><dt>Расчётный месяц (МСК)</dt><dd>{}</dd>\
                 <dt>Оценка расходов</dt><dd>{}</dd>\
                 <dt>Остаток бюджета</dt><dd>{}</dd>\
                 <dt>Зарезервировано попыток</dt><dd>{}</dd>\
                 <dt>Учтённая бесплатная квота</dt><dd>{}</dd>\
                 <dt>Остаток бесплатной квоты</dt><dd>{}</dd></dl>\
                 <p><strong>{warning}</strong></p>",
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
         Включение почты и сверка месяца пока недоступны из этой формы. \
         Пороговые предупреждения отображаются здесь, внешняя доставка уведомлений не подключена.</p>\
         </div></section>",
        view.revision,
        l.warning_1 / 100,
        l.warning_2 / 100,
        l.news_pause / 100,
        l.monthly_limit / 100,
    )
}

#[cfg(test)]
mod tests;
