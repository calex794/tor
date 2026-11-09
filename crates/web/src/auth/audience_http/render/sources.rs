//! Fixed anonymous channels only. Missing measurement remains visibly missing.
use super::time;
use crate::pages::escape_html;
use std::fmt::Write;
use tor_recommendations::services::audience::{
    DiscoveryChannel, DiscoveryPeriod, DiscoveryReport, DiscoverySource,
};

pub(super) fn section(body: &mut String, report: &DiscoveryReport) {
    body.push_str(
        "<section><h2>Источники первого чтения книги</h2>\
         <p>Каталог, поиск по сайту и рекомендации: учтённые переходы, открытия книги \
         и первые подтверждённые события чтения при согласии читателя.</p>\
         <p>Здесь сравниваются календарные дни UTC. Начало включено, конец не включён. \
         Сегодняшний день ещё не завершён.</p>",
    );
    write!(
        body,
        "<p>Время расчёта: {}.</p>",
        time(&report.measured_at_utc)
    )
    .expect("String");
    health(body, report);
    period(body, "current", "Текущий период", &report.current);
    period(body, "previous", "Предыдущий период", &report.previous);
    body.push_str(
        "<details class=\"book-section-card\"><summary>Как читать показатели источников</summary>\
         <p>Для каталога, поиска и рекомендаций день относится к выдаче учтённой карточки. \
         Переход, открытие и чтение могут произойти позднее. \
         Для неизвестного источника используется день самого события чтения.</p>\
         <p>Первое чтение учитывается один раз для учётной записи и книги. \
         Чтение следующих глав не увеличивает это число. Первое завершение главы в книге \
         не означает, что книга прочитана целиком.</p>\
         <p>Для каждого события выбирается один последний подходящий переход: \
         в пределах 7 дней до начала или 14 дней до завершения. \
         У начала и завершения одной книги источники могут различаться. \
         Рекомендации могут получить подтверждённое чтение после перехода \
         без отдельного подтверждения открытия.</p>\
         <p>Поэтому эти числа не образуют строгую последовательность. \
         Их отношения не показывают долю посетителей, начавших или закончивших чтение. \
         Сводка глав ниже измеряет другие события и другой временной интервал.</p>\
         <p>«Нет данных» означает отсутствие сохранённого измерения или неполное покрытие. \
         Старые открытия из рекомендаций могли не измеряться. \
         Ноль показывается только при наличии измеренных данных. \
         Неизвестный источник не является отдельным каналом перехода.</p>\
         <p>Отзыв согласия останавливает новый сбор. Недавние сведения убираются \
         после обновления итогов; обезличенные исторические итоги сохраняются \
         в пределах действующего срока хранения.</p></details></section>",
    );
}

fn health(body: &mut String, report: &DiscoveryReport) {
    let health = &report.health;
    body.push_str("<section class=\"book-section-card\" data-testid=\"audience-sources-health\"><h3>Обновление данных</h3>");
    if let Some(last_success) = &health.last_success_utc {
        write!(
            body,
            "<p>Последнее подтверждение обработки: {}.</p>",
            time(last_success)
        )
        .expect("String");
    } else {
        body.push_str("<p>Обработка ещё не подтверждена. Полнота свежих данных неизвестна.</p>");
    }
    if !health.fresh {
        body.push_str(
            "<p>Нет подтверждения за последние 30 секунд. Показанные итоги могут отставать.</p>",
        );
    } else {
        body.push_str("<p>Обработка подтверждена за последние 30 секунд.</p>");
    }
    if health.rec_dirty || health.organic_dirty {
        body.push_str(
            "<p>Есть изменения, ожидающие пересчёта. Итоги могут измениться после обработки.</p>",
        );
    }
    body.push_str("</section>");
}

fn period(body: &mut String, key: &str, label: &str, period: &DiscoveryPeriod) {
    write!(
        body,
        "<section data-testid=\"audience-sources-{key}\"><h3>{label}</h3><p>С {} до {}.</p>",
        day(&period.from_day),
        day(&period.until_day_exclusive)
    )
    .expect("String");
    if period.includes_partial_today {
        body.push_str("<p>Включает незавершённый сегодняшний день.</p>");
    }
    body.push_str("<div class=\"audience-source-grid\">");
    for channel in &period.channels {
        channel_card(body, channel);
    }
    body.push_str("</div></section>");
}

fn day(value: &str) -> String {
    let escaped = escape_html(value);
    format!("<time datetime=\"{escaped}\">{escaped} UTC</time>")
}

fn channel_card(body: &mut String, channel: &DiscoveryChannel) {
    let (key, label) = match channel.source {
        DiscoverySource::Catalog => ("catalog", "Каталог"),
        DiscoverySource::Search => ("search", "Поиск по сайту"),
        DiscoverySource::Recommendations => ("recommendations", "Рекомендации"),
        DiscoverySource::Unattributed => ("unattributed", "Источник не установлен"),
    };
    write!(body, "<article class=\"book-section-card audience-source-card\" data-source=\"{key}\"><h4>{label}</h4><dl>").expect("String");
    for (metric, label, value) in [
        ("clicks", "Учтённые переходы", channel.clicks),
        ("book-opens", "Открытия книги", channel.book_opens),
        ("starts", "Первые чтения книг", channel.starts),
        (
            "completions",
            "Первое завершение главы в книге",
            channel.completions,
        ),
    ] {
        let value = if channel.source == DiscoverySource::Unattributed
            && matches!(metric, "clicks" | "book-opens")
        {
            "Не измеряется".to_owned()
        } else {
            value.map_or_else(|| "Нет данных".to_owned(), |value| value.to_string())
        };
        write!(
            body,
            "<dt>{label}</dt><dd data-metric=\"{metric}\">{value}</dd>"
        )
        .expect("String");
    }
    body.push_str("</dl></article>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_rec_opens_and_measured_zero_are_distinct() {
        let mut html = String::new();
        channel_card(
            &mut html,
            &DiscoveryChannel {
                source: DiscoverySource::Recommendations,
                clicks: Some(0),
                book_opens: None,
                starts: Some(2),
                completions: Some(1),
            },
        );
        assert!(html.contains("data-metric=\"clicks\">0</dd>"));
        assert!(html.contains("data-metric=\"book-opens\">Нет данных</dd>"));
        assert!(html.contains("data-metric=\"starts\">2</dd>"));
        assert!(!html.contains("book_id"));
        assert!(!html.contains("user_id"));
    }

    #[test]
    fn unknown_source_does_not_claim_navigation_measurement() {
        let mut html = String::new();
        channel_card(
            &mut html,
            &DiscoveryChannel {
                source: DiscoverySource::Unattributed,
                clicks: None,
                book_opens: None,
                starts: Some(3),
                completions: None,
            },
        );
        assert_eq!(html.matches("Не измеряется").count(), 2);
        assert!(html.contains("data-metric=\"starts\">3</dd>"));
        assert!(html.contains("data-metric=\"completions\">Нет данных</dd>"));
    }

    #[test]
    fn calendar_dates_are_escaped_in_text_and_attributes() {
        assert_eq!(
            day("2026-11-10"),
            "<time datetime=\"2026-11-10\">2026-11-10 UTC</time>"
        );
        let html = day("\"><script>bad()</script>");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&quot;&gt;&lt;script&gt;"));
    }
}
