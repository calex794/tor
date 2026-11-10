//! FLOW-06: local capture and persistence evidence, never a coverage percentage.
//! No user identifiers or raw request fields are read or rendered.
use crate::pages::escape_html;
use std::fmt::Write;
use tor_traffic_behavior::services::{audience::FlowHourlyReport, PgPumpSnapshot};

const STALE_SECONDS: u64 = 90;

pub(super) fn section(
    body: &mut String,
    days: i32,
    report: Option<&FlowHourlyReport>,
    capture: Option<(u64, u64)>,
    pump: Option<PgPumpSnapshot>,
) {
    body.push_str(
        "<section class=\"book-section-card\" data-testid=\"audience-data-quality\">\
         <h2>Недостоверность измерений</h2>\
         <p>Состояние сбора и пропуски показаны отдельно от посещений. \
         Эти сведения относятся только к текущему веб-процессу; \
         другие процессы и потери до запуска неизвестны.</p>",
    );
    match capture {
        Some((candidates, dropped)) => {
            write!(
                body,
                "<dl><dt>Подходящие запросы с запуска процесса</dt>\
                 <dd data-metric=\"flow-candidates\">{candidates}</dd>\
                 <dt>Потеряно до очереди</dt>\
                 <dd data-metric=\"flow-dropped\">{dropped}</dd></dl>",
            )
            .expect("String");
            if dropped > 0 {
                body.push_str(
                    "<p role=\"alert\">Есть потери перед записью: сохранённые просмотры \
                     нельзя считать полным охватом.</p>",
                );
            }
        }
        None => body.push_str(
            "<p role=\"alert\">Локальный сборщик недоступен: нет измерения \
             подходящих запросов и потерь, а не нулевая посещаемость.</p>",
        ),
    }

    match pump {
        Some(pump) => {
            write!(
                body,
                "<p>Циклов обработки очереди: <span data-metric=\"pump-cycles\">{}</span>; \
                 с ошибками: <span data-metric=\"pump-failures\">{}</span>; \
                 подряд с ошибками: <span data-metric=\"pump-failure-streak\">{}</span>.</p>",
                pump.cycles, pump.failed_cycles, pump.consecutive_failures,
            )
            .expect("String");
            match pump.age_since_last_success_s {
                Some(seconds) => {
                    write!(
                        body,
                        "<p>Последний успешный цикл обработки очереди: \
                         <span data-metric=\"pump-last-success-age\">{seconds}</span> секунд назад. \
                         Успешный цикл мог быть пустым: это не подтверждает сохранение просмотров.</p>",
                    )
                    .expect("String");
                }
                None => body.push_str(
                    "<p role=\"alert\">Успешный цикл записи ещё не подтверждён: \
                     нет измерения его времени, не ноль сохранённых просмотров.</p>",
                ),
            }
            let stale = pump.running_for_s.is_some_and(|s| s >= STALE_SECONDS)
                || pump
                    .age_since_last_completion_s
                    .is_some_and(|s| s >= STALE_SECONDS)
                || (pump.cycles == 0 && pump.age_since_start_s.is_some_and(|s| s >= STALE_SECONDS))
                || pump
                    .age_since_last_success_s
                    .is_some_and(|s| s >= STALE_SECONDS);
            if stale || pump.consecutive_failures > 0 {
                body.push_str(
                    "<p role=\"alert\">Фоновая запись могла отстать или завершаться с ошибками. \
                     Текущие данные могут быть неполными; проверьте операторскую панель.</p>",
                );
            }
        }
        None => body.push_str(
            "<p role=\"alert\">Нет сведений о локальной фоновой записи в PostgreSQL: \
             успешная обработка не подтверждена.</p>",
        ),
    }

    if matches!(days, 1 | 7) {
        let last = report.and_then(|report| {
            report
                .current
                .hours
                .iter()
                .rev()
                .chain(report.previous.hours.iter().rev())
                .find(|hour| hour.views.iter().any(Option::is_some))
        });
        if let Some(hour) = last {
            write!(
                body,
                "<p>Последний завершённый час со строкой публичных просмотров в PostgreSQL: \
                 <time data-testid=\"audience-last-saved-hour\" datetime=\"{}\">{} UTC</time>. \
                 Это час записи пакета, а не время посещения.</p>",
                escape_html(&hour.hour_utc),
                escape_html(&hour.hour_utc.trim_end_matches('Z').replace('T', " ")),
            )
            .expect("String");
        } else {
            body.push_str(
                "<p data-testid=\"audience-no-saved-hour\">Нет подтверждённого часа \
                 со строкой просмотров в выбранных интервалах. \
                 Это не означает ноль посещений.</p>",
            );
        }
    } else {
        body.push_str(
            "<p>Почасовой источник для 30/90 дней не запрашивался. \
             Посмотреть наличие сохранённых часов можно за сутки или неделю.</p>",
        );
    }
    body.push_str(
        "<p>Нельзя делить сохранённые просмотры на локальные кандидаты: \
         у чисел разные периоды, а часть пакетов может сохраняться с задержкой. \
         Никакая сводка здесь не доказывает точную посещаемость или число людей.</p></section>",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_traffic_behavior::services::audience::{FlowHour, FlowHourlyPeriod};

    fn sample_report(hour_utc: &str) -> FlowHourlyReport {
        let hour = FlowHour {
            hour_epoch: 3_600,
            hour_utc: hour_utc.to_owned(),
            views: [Some(0), None, None, None],
            opted_in_html_requests: [Some(0), None, None, None],
            observed_visit_starts: [Some(0), None, None, None],
        };
        let period = FlowHourlyPeriod {
            from_epoch: 0,
            until_epoch: 7_200,
            from_utc: "2026-11-10T00:00:00Z".into(),
            until_utc: "2026-11-10T02:00:00Z".into(),
            hours: vec![hour],
            saved_views: [Some(0), None, None, None],
            saved_opted_in_requests: [Some(0), None, None, None],
            observed_visit_starts: [Some(0), None, None, None],
            recorded_hours: [1, 0, 0, 0],
        };
        FlowHourlyReport {
            days: 1,
            measured_at_epoch: 7_201,
            measured_at_utc: "2026-11-10T02:00:01Z".into(),
            current: period.clone(),
            previous: period,
        }
    }

    #[test]
    fn absence_is_unknown_and_never_a_measured_zero() {
        let mut body = String::new();
        section(&mut body, 7, None, None, None);
        assert!(body.contains("audience-no-saved-hour"));
        assert!(body.contains("нет измерения"));
        assert!(body.contains("не нулевая посещаемость"));
        assert!(!body.contains("data-metric=\"flow-dropped\">0"));
        assert!(!body.contains("pump-cycles"));
    }

    #[test]
    fn zero_loss_and_saved_zero_are_distinct_from_missing_data() {
        let report = sample_report("2026-11-10T01:00:00Z");
        let pump = PgPumpSnapshot {
            cycles: 2,
            age_since_last_success_s: Some(1),
            ..PgPumpSnapshot::default()
        };
        let mut body = String::new();
        section(&mut body, 1, Some(&report), Some((4, 0)), Some(pump));
        assert!(body.contains("data-metric=\"flow-dropped\">0"));
        assert!(body.contains("audience-last-saved-hour"));
        assert!(!body.contains("audience-no-saved-hour"));
        assert!(!body.contains("Фоновая запись могла отстать"));
        assert!(!body.contains("нулевая посещаемость"));
    }

    #[test]
    fn degraded_or_stale_worker_and_loss_are_explicit_warnings() {
        let pump = PgPumpSnapshot {
            cycles: 5,
            failed_cycles: 3,
            consecutive_failures: 2,
            age_since_last_success_s: Some(120),
            ..PgPumpSnapshot::default()
        };
        let mut body = String::new();
        section(&mut body, 30, None, Some((10, 3)), Some(pump));
        assert!(body.contains("data-metric=\"flow-dropped\">3"));
        assert!(body.contains("Фоновая запись могла отстать"));
        assert!(body.contains("30/90"));
        assert!(!body.contains("audience-last-saved-hour"));
        assert_eq!(body.matches("role=\"alert\"").count(), 2);
    }

    #[test]
    fn failed_completion_does_not_look_like_success_or_allow_html_injection() {
        let pump = PgPumpSnapshot {
            cycles: 2,
            failed_cycles: 2,
            consecutive_failures: 2,
            age_since_last_completion_s: Some(1),
            age_since_last_success_s: None,
            ..PgPumpSnapshot::default()
        };
        let mut body = String::new();
        let report = sample_report("\"><script>alert(1)</script>");
        section(&mut body, 7, Some(&report), Some((0, 0)), Some(pump));
        assert!(body.contains("Успешный цикл записи ещё не подтверждён"));
        assert!(!body.contains("<script>"));
        assert!(body.contains("&lt;script&gt;"));
    }
}
