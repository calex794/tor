//! Provider-reported aggregates, with explicit calendars, coverage and non-attribution.
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_search::domain::seo_analytics::*;
fn number(n: Option<i64>) -> String {
    n.map(|n| n.to_string())
        .unwrap_or_else(|| "Нет данных".into())
}
fn ratio(clicks: Option<i64>, impressions: Option<i64>) -> String {
    match (clicks, impressions) {
        (Some(c), Some(i)) if i > 0 => format!("{:.2}%", c as f64 * 100.0 / i as f64),
        _ => "Не рассчитан".into(),
    }
}
fn period(body: &mut String, key: &str, p: &PeriodTotals, days: i32) {
    write!(body,"<section class=\"book-section-card\" data-testid=\"seo-{key}\"><h2>{}</h2><p>{} &mdash; {} (конец не включён)</p>",if key=="current"{"Текущий период"}else{"Предыдущий период"},escape_html(&p.from),escape_html(&p.until)).unwrap();
    write!(body,"<p>Дней с итогами: {} / {days}; с лимитом строк: {}. Отброшено неканонических URL: {}.</p><dl>",p.total_days,p.limited_days,p.omitted_rows).unwrap();
    for (metric, label, value) in [
        ("clicks", "Клики", number(p.clicks)),
        ("impressions", "Показы", number(p.impressions)),
        ("ctr", "CTR", ratio(p.clicks, p.impressions)),
        (
            "position",
            "Средняя позиция",
            p.position
                .map(|n| format!("{n:.2}"))
                .unwrap_or_else(|| number(None)),
        ),
    ] {
        write!(
            body,
            "<dt>{label}</dt><dd data-metric=\"{metric}\">{value}</dd>"
        )
        .unwrap();
    }
    body.push_str(
        "</dl><p>Сумма только по полученным дням. Пропуски не заменяются нулями.</p></section>",
    );
}
pub(in crate::auth::audience_http) fn page(
    days: i32,
    provider: Provider,
    report: Option<&SeoReport>,
    csrf: &str,
) -> Html<String> {
    let mut body=String::from("<section class=\"section\"><div class=\"container author-width\"><h1>SEO: поисковый трафик</h1><p>Показы и клики из поисковых кабинетов, не посещения TOR и не чтение.</p><form method=\"get\" action=\"/admin/audience/seo\" class=\"author-form\"><label>Источник<select name=\"provider\">");
    for (value, label) in [
        (Provider::Google, "Google Search Console"),
        (Provider::Yandex, "Яндекс Вебмастер"),
    ] {
        write!(
            body,
            "<option value=\"{}\"{}>{label}</option>",
            value.key(),
            if provider == value { " selected" } else { "" }
        )
        .unwrap();
    }
    body.push_str("</select></label><label>Дней<select name=\"days\">");
    for n in [1, 7, 30, 90] {
        write!(
            body,
            "<option value=\"{n}\"{}>{n}</option>",
            if n == days { " selected" } else { "" }
        )
        .unwrap();
    }
    body.push_str("</select></label><button class=\"button\">Показать</button></form>");
    if let Some(r) = report {
        let configured = r.sources.iter().find(|s| s.source.key.provider == provider);
        if let Some(s) = configured {
            write!(body,"<section class=\"book-section-card\" data-testid=\"seo-source\"><h2>{}</h2><p>Календарь: {}. Последняя загрузка: {}.</p><p>Ожидают: {}; в работе: {}; ошибка: {}.</p><form method=\"post\" action=\"/admin/audience/seo/refresh\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"provider\" value=\"{}\"><button class=\"button\">Обновить последние 7 дат</button></form></section>",escape_html(&s.source.key.property),provider.calendar(),escape_html(s.last_success.as_deref().unwrap_or("Не загружено")),s.pending,s.running,escape_html(s.last_error.as_deref().unwrap_or("нет")),escape_html(csrf),provider.key()).unwrap();
        } else {
            body.push_str(
                "<p data-testid=\"seo-unconfigured\">Источник не подключён. Данные неизвестны.</p>",
            );
        }
        period(&mut body, "current", &r.current, days);
        period(&mut body, "previous", &r.previous, days);
        if r.current.total_days == i64::from(days) && r.previous.total_days == i64::from(days) {
            if let (Some(a), Some(b)) = (r.current.clicks, r.previous.clicks) {
                write!(
                    body,
                    "<p data-testid=\"seo-comparison\">Изменение кликов: {:+}.</p>",
                    a - b
                )
                .unwrap();
            }
        } else {
            body.push_str("<p data-testid=\"seo-comparison-unavailable\">Сравнение недоступно: покрытие дат неполное.</p>");
        }
        body.push_str("<h2>Посадочные страницы</h2>");
        for p in &r.pages {
            write!(body,"<article class=\"book-section-card\" data-testid=\"seo-page\"><h3><a href=\"{}\">{}</a></h3><p>{}</p><dl><dt>Клики</dt><dd>{}</dd><dt>Показы</dt><dd>{}</dd><dt>CTR</dt><dd>{}</dd><dt>Средняя позиция</dt><dd>{}</dd><dt>Дней со строкой</dt><dd>{}</dd></dl></article>",escape_html(&p.path),escape_html(&p.title),escape_html(&p.path),p.clicks,p.impressions,ratio(Some(p.clicks),Some(p.impressions)),p.position.map(|v|format!("{v:.2}")).unwrap_or_else(||number(None)),p.measured_days).unwrap();
        }
        if r.pages.is_empty() {
            body.push_str(
                "<p>Сохранённых доступных строк нет; это не доказывает отсутствие трафика.</p>",
            );
        }
        if let Some(next) = &r.next {
            write!(body,"<a class=\"button\" data-testid=\"seo-next\" href=\"/admin/audience/seo?provider={}&amp;days={days}&amp;after={}\">Далее</a>",provider.key(),escape_html(next)).unwrap();
        }
    } else {
        body.push_str("<p role=\"alert\" data-testid=\"seo-unavailable\">Источник временно недоступен. Нули не подставляются.</p>");
    }
    body.push_str("<section class=\"book-section-card\"><h2>Границы измерения</h2><p>Google и Яндекс не складываются. Итоги сайта и строки URL имеют разную агрегацию. Лимиты провайдера могут скрывать строки. Нет связи клика с отдельным читателем, покупкой или чтением. Текущая дата и две предыдущие не включены из-за задержки источника.</p></section><p><a href=\"/admin/audience\">Аудитория</a></p></div></section>");
    layout("SEO", "Закрытая поисковая аналитика", &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_is_not_zero_and_partial_periods_are_not_comparable() {
        let html = page(7, Provider::Google, None, "safe").0;
        assert!(html.contains("seo-unavailable"));
        assert!(!html.contains("data-metric="));
        let p = PeriodTotals {
            from: "2026-11-01".into(),
            until: "2026-11-08".into(),
            measured_days: 1,
            total_days: 1,
            clicks: Some(0),
            impressions: Some(0),
            position: None,
            limited_days: 0,
            omitted_rows: 0,
        };
        let report = SeoReport {
            days: 7,
            provider: Provider::Google,
            sources: vec![],
            current: p.clone(),
            previous: p,
            pages: vec![ReportPage {
                path: "/books/7".into(),
                title: "<script>bad()</script>".into(),
                clicks: 1,
                impressions: 10,
                position: Some(3.25),
                measured_days: 1,
            }],
            next: None,
            measured_at: "2026-11-11T00:00:00Z".into(),
        };
        let html = page(7, Provider::Google, Some(&report), "safe").0;
        assert!(html.contains("seo-comparison-unavailable"));
        assert!(html.contains("data-metric=\"clicks\">0"));
        assert!(html.contains("3.25"));
        assert!(!html.contains("<script>bad()"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
