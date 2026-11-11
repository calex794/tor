//! Numerical FLOW evidence behind the existing operator authentication.
use std::fmt::Write;
use tor_traffic_behavior::domain::FlowHourlyCount;

pub(super) fn render(rows: Option<&[FlowHourlyCount]>, capture: Option<(u64, u64)>) -> String {
    let mut html = String::from(
        "<section class=\"card\" aria-labelledby=\"flow\"><h2 id=\"flow\">Посещения страниц</h2>\
         <p>Сохранённые данные за 24 часовые корзины UTC, включая текущий неполный час.</p>",
    );
    match capture {
        Some((candidates, dropped)) => {
            write!(html, "<dl><dt>Допустимые просмотры с запуска процесса</dt><dd>{candidates}</dd><dt>Потеряно до очереди</dt><dd>{dropped}</dd></dl>")
                .expect("write String");
            if dropped > 0 {
                html.push_str(
                    "<p role=\"alert\">Часть просмотров потеряна. Сохранённые данные неполны.</p>",
                );
            }
        }
        None => html.push_str("<p>Сборщик просмотров не запущен: нет измерения.</p>"),
    }
    match rows {
        None => html.push_str("<p>Сохранённые просмотры недоступны: нет измерения.</p>"),
        Some([]) => html.push_str("<p>За выбранный период нет сохранённых измерений. Это не означает отсутствие посетителей.</p>"),
        Some(rows) => {
            let mut surfaces = [0_i128; 5];
            let mut sources = [0_i128; 4];
            let mut opted_in = 0_i128;
            let mut visits = 0_i128;
            for row in rows {
                let surface = usize::try_from(row.surface).ok().filter(|i| *i < 5);
                let source = usize::try_from(row.acquisition_hint).ok().filter(|i| *i < 4);
                let (Some(surface), Some(source)) = (surface, source) else {
                    return unavailable();
                };
                if row.html_requests < 0
                    || row.opted_in_html_requests < 0
                    || row.observed_opted_in_visit_starts < 0
                    || row.opted_in_html_requests > row.html_requests
                    || row.observed_opted_in_visit_starts > row.opted_in_html_requests
                {
                    return unavailable();
                }
                surfaces[surface] += i128::from(row.html_requests);
                sources[source] += i128::from(row.html_requests);
                opted_in += i128::from(row.opted_in_html_requests);
                visits += i128::from(row.observed_opted_in_visit_starts);
            }
            write!(html, "<dl><dt>Сохранённые просмотры страниц</dt><dd>{}</dd><dt>Из них с согласием на учёт</dt><dd>{opted_in}</dd><dt>Наблюдаемые начала визитов с согласием</dt><dd>{visits}</dd></dl>", surfaces.iter().sum::<i128>())
                .expect("write String");
            list(&mut html, "Страницы", &["Главная", "Каталог", "Книга", "Читалка", "Личный кабинет"], &surfaces);
            list(&mut html, "Подсказки источника перехода", &["Внешний поиск", "Внешняя ссылка", "Платный источник", "Прямой или неизвестный"], &sources);
        }
    }
    html.push_str("<p>Просмотры и начала визитов не равны уникальным людям. После перезапуска, вытеснения состояния или отзыва согласия сомнительные новые визиты не учитываются 30 минут; такие пропуски не означают ноль посещений. При нескольких серверах общая уникальность не подтверждена. Источник перехода — непроверенная подсказка, не подтверждение рекламы или чтения. Отсутствующие срезы не заменяются нулём.</p></section>");
    html
}

fn unavailable() -> String {
    "<section class=\"card\"><h2>Посещения страниц</h2><p role=\"alert\">Некорректные агрегаты: нет достоверного измерения.</p></section>".to_owned()
}

fn list(html: &mut String, title: &str, labels: &[&str], counts: &[i128]) {
    write!(html, "<h3>{title}</h3><dl>").expect("write String");
    for (label, count) in labels.iter().zip(counts) {
        if *count > 0 {
            write!(html, "<dt>{label}</dt><dd>{count}</dd>").expect("write String");
        }
    }
    html.push_str("</dl>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> FlowHourlyCount {
        FlowHourlyCount {
            hour_epoch: 0,
            surface: 3,
            acquisition_hint: 0,
            html_requests: 10,
            opted_in_html_requests: 6,
            observed_opted_in_visit_starts: 2,
        }
    }

    #[test]
    fn missing_storage_is_not_zero_audience() {
        assert!(render(None, None).contains("нет измерения"));
        let empty = render(Some(&[]), Some((0, 0)));
        assert!(empty.contains("не означает отсутствие посетителей"));
        assert!(!empty.contains("Сохранённые просмотры страниц</dt><dd>0"));
    }

    #[test]
    fn recorded_requests_are_separate_from_local_losses_and_visits() {
        let html = render(Some(&[row(), row()]), Some((100, 7)));
        assert!(html.contains("Сохранённые просмотры страниц</dt><dd>20"));
        assert!(html.contains("Читалка</dt><dd>20"));
        assert!(html.contains("Внешний поиск</dt><dd>20"));
        assert!(html.contains("визитов с согласием</dt><dd>4"));
        assert!(html.contains("Потеряно до очереди</dt><dd>7"));
        assert!(html.contains("role=\"alert\""));
        assert!(!html.contains("Каталог</dt><dd>0"));
    }

    #[test]
    fn invalid_rows_fail_closed_instead_of_inventing_totals() {
        let mut invalid = row();
        invalid.surface = -1;
        assert!(render(Some(&[invalid]), None).contains("Некорректные агрегаты"));
        invalid = row();
        invalid.observed_opted_in_visit_starts = 7;
        assert!(render(Some(&[invalid]), None).contains("Некорректные агрегаты"));
    }
}
