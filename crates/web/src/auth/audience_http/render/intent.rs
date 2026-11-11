//! Explicit C choices: current state and surviving creation cohorts, not a funnel.
use std::fmt::Write;
use tor_reading::services::audience::intent::{IntentCounts, IntentReport};

pub(super) fn section(body: &mut String, report: Option<&IntentReport>) {
    body.push_str("<section class=\"book-section-card\" data-testid=\"audience-intents\"><h2>Библиотеки и подписки на книги</h2>");
    let Some(report) = report else {
        body.push_str("<p role=\"alert\" data-testid=\"audience-intents-unavailable\">Сводка библиотек и подписок временно недоступна. Нулевые значения не подставляются.</p></section>");
        return;
    };
    write!(body, "<p>Состояние на {}.</p><p>Учитываются только существующие записи. Удалённые сохранения и отменённые подписки не восстанавливаются. Это не полная история действий.</p>", super::time(&report.measured_at_utc)).expect("String");
    counts(body, "total", "Сейчас всего", &report.totals);
    for (key, title, from, until, values) in [
        (
            "current",
            "Созданы в текущем периоде и сохранены сейчас",
            &report.current_from_utc,
            &report.measured_at_utc,
            &report.current,
        ),
        (
            "previous",
            "Созданы в предыдущем периоде и сохранены сейчас",
            &report.previous_from_utc,
            &report.current_from_utc,
            &report.previous,
        ),
    ] {
        write!(body, "<section data-testid=\"audience-intents-{key}-period\"><h3>{title}</h3><p>{} &mdash; {}. Начало включено, конец не включён.</p>", super::time(from),super::time(until)).expect("String");
        counts(body, key, "Оставшиеся записи", values);
        body.push_str("</section>");
    }
    body.push_str("<p>Эти числа не показывают конверсию, отток или дочитывание книг. Аккаунты считаются отдельно в каждой группе; складывать их как уникальных посетителей нельзя.</p></section>");
}

fn counts(body: &mut String, key: &str, title: &str, values: &IntentCounts) {
    write!(
        body,
        "<div data-testid=\"audience-intents-{key}\"><h3>{title}</h3><dl>"
    )
    .expect("String");
    for (metric, label, value) in [
        (
            "library-links",
            "Записей книг в личных библиотеках",
            values.library_links,
        ),
        (
            "library-accounts",
            "Учётных записей с этими сохранениями",
            values.library_accounts,
        ),
        (
            "follow-links",
            "Действующих подписок на книги",
            values.follow_links,
        ),
        (
            "follow-accounts",
            "Учётных записей с этими подписками",
            values.follow_accounts,
        ),
    ] {
        write!(
            body,
            "<dt>{label}</dt><dd data-metric=\"{metric}\">{value}</dd>"
        )
        .expect("String");
    }
    body.push_str("</dl></div>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_is_not_zero_and_snapshot_is_not_a_historical_funnel() {
        let mut html = String::new();
        section(&mut html, None);
        assert!(html.contains("audience-intents-unavailable"));
        assert!(!html.contains("data-metric="));
        let report = IntentReport {
            days: 7,
            measured_at_utc: "2026-11-11T12:00:00Z".into(),
            current_from_utc: "2026-11-04T12:00:00Z".into(),
            previous_from_utc: "\"><script>bad()</script>".into(),
            totals: IntentCounts {
                library_links: 8,
                library_accounts: 3,
                follow_links: 4,
                follow_accounts: 2,
            },
            current: IntentCounts::default(),
            previous: IntentCounts::default(),
        };
        html.clear();
        section(&mut html, Some(&report));
        assert!(html.contains("data-metric=\"library-links\">8</dd>"));
        assert!(html.contains("data-metric=\"follow-links\">0</dd>"));
        assert!(html.contains("Учитываются только существующие записи. Удалённые сохранения и отменённые подписки не восстанавливаются. Это не полная история действий."));
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        for secret in ["user_id", "book_id", "email", "subject"] {
            assert!(!html.contains(secret));
        }
    }
}
