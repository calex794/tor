use std::fmt::Write;
use tor_traffic_behavior::{
    domain::html_health::{HtmlHealthPeriod, HtmlHealthReport},
    services::observed_audience::ObservedAudienceReport,
};

pub(super) fn complete(period: Option<HtmlHealthPeriod>, expected: usize) -> bool {
    expected > 0
        && period.is_some_and(|period| {
            usize::try_from(period.confirmed_hours) == Ok(expected) && !period.has_failure()
        })
}

pub(super) fn section(
    body: &mut String,
    counts: Option<&ObservedAudienceReport>,
    quality: Option<&HtmlHealthReport>,
) {
    body.push_str("<section class=\"book-section-card\" data-testid=\"audience-durable-quality\"><h2>Подтверждение записи HTML</h2><p>Качество двух соседних периодов завершённых UTC-часов. Проверяется, что все зарегистрированные процессы сбора передали очередь и подтвердили запись. Часы с записями и локальные счётчики сами по себе этого не доказывают.</p>");
    let Some(counts) = counts else {
        unavailable(body);
        return;
    };
    let Some(quality) = quality else {
        body.push_str("<p role=\"alert\" data-testid=\"audience-html-health-missing\">Журнал качества ещё не установлен. Сохранённые числа доступны, но полнота периода и отсутствие потерь неизвестны.</p></section>");
        return;
    };
    if quality.until_epoch != counts.current.until_epoch {
        unavailable(body);
        return;
    }
    for (key, title, period) in [
        ("current", "Текущий период", quality.current),
        ("previous", "Предыдущий период", quality.previous),
    ] {
        let complete = complete(Some(period), counts.days as usize * 24);
        let state = if complete { "confirmed" } else { "unknown" };
        write!(
            body,
            "<section data-health-period=\"{key}\" data-health-state=\"{state}\"><h3>{title}</h3><dl><dt>Подтверждённые часы</dt><dd data-metric=\"html-confirmed-hours\">{} из {}</dd><dt>Часы с известными потерями</dt><dd data-metric=\"html-loss-hours\">{}</dd><dt>Часы с ошибками записи</dt><dd data-metric=\"html-error-hours\">{}</dd></dl>",
            period.confirmed_hours,
            counts.days * 24,
            period.loss_hours,
            period.error_hours,
        )
        .expect("String");
        if complete {
            body.push_str("<p>Все часы этого периода подтверждены; зарегистрированных потерь и ошибок нет.</p>");
        } else {
            body.push_str("<p role=\"alert\">Период подтверждён не целиком либо содержит потери. Сравнение объёма не подтверждено.</p>");
        }
        body.push_str("</section>");
    }
    body.push_str("<p>Это свидетельство зарегистрированных процессов, не полнота всей установки и не число людей. Пропавший процесс остаётся неизвестным после истечения аренды; новый процесс не восстанавливает его историю. Ноль известных ошибок не доказывает их отсутствие в неизвестном интервале.</p><p>Если полный период подтверждён без потерь, отсутствие сохранённых ответов показано нулём. Иначе пропуск остаётся «Нет измерения». Часы с записями по-прежнему обозначают наличие строк, а не полноту сбора.</p></section>");
}

fn unavailable(body: &mut String) {
    body.push_str("<p role=\"alert\" data-testid=\"audience-html-health-unavailable\">Долговечная диагностика недоступна. Полнота периода и отсутствие потерь не подтверждены; нули не подставляются.</p></section>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_inference_needs_a_complete_nonempty_window_without_losses() {
        assert!(!complete(None, 24));
        assert!(!complete(Some(HtmlHealthPeriod::default()), 24));
        assert!(!complete(Some(HtmlHealthPeriod::default()), 0));
        let full = HtmlHealthPeriod {
            confirmed_hours: 24,
            ..HtmlHealthPeriod::default()
        };
        assert!(complete(Some(full), 24));
        assert!(!complete(Some(full), 168));
        assert!(!complete(
            Some(HtmlHealthPeriod { loss_hours: 1, ..full }),
            24,
        ));
        assert!(!complete(
            Some(HtmlHealthPeriod { error_hours: 1, ..full }),
            24,
        ));
    }
}
