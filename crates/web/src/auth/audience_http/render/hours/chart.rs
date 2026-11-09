//! A bounded, script-free graph; gaps break the line rather than imply zero views.
use std::fmt::Write;
use tor_traffic_behavior::services::audience::{FlowHour, FlowHourlyReport};

pub(super) fn render(body: &mut String, report: &FlowHourlyReport) {
    let current = totals(&report.current.hours);
    let previous = totals(&report.previous.hours);
    let ceiling = current
        .iter()
        .chain(&previous)
        .flatten()
        .copied()
        .max()
        .unwrap_or(0)
        .max(1);
    body.push_str(
        "<figure class=\"audience-hours-chart\"><figcaption>Сохранённые просмотры четырёх категорий по часам</figcaption>\
         <p class=\"audience-hours-legend\"><span class=\"audience-hours-current-key\">Текущий период</span> \
         <span class=\"audience-hours-previous-key\">Предыдущий период</span></p>\
         <svg viewBox=\"0 0 640 128\" role=\"img\" aria-labelledby=\"audience-hours-chart-title\">\
         <title id=\"audience-hours-chart-title\">Сравнение сохранённых просмотров. \
         Разрывы линий означают отсутствие записей; точные числа приведены в таблице ниже.</title>\
         <path d=\"M30 88H630\" class=\"audience-hours-axis\"/>",
    );
    write!(
        body,
        "<text x=\"30\" y=\"12\">{ceiling}</text><text x=\"15\" y=\"92\">0</text>"
    )
    .expect("String");
    series(body, &current, ceiling, "audience-hours-line-current");
    series(body, &previous, ceiling, "audience-hours-line-previous");
    if let (Some(first), Some(last)) = (report.current.hours.first(), report.current.hours.last()) {
        write!(body, "<text x=\"30\" y=\"116\">{}</text><text x=\"630\" y=\"116\" text-anchor=\"end\">{}</text>",
            super::short_hour(&first.hour_utc), super::short_hour(&last.hour_utc)).expect("String");
    }
    body.push_str("</svg><p>Пустые участки — нет записей. Точки показывают сумму сохранённых категорий; она может быть неполной.</p></figure>");
}

fn totals(hours: &[FlowHour]) -> Vec<Option<i128>> {
    hours
        .iter()
        .map(|hour| {
            let mut values = hour.views.iter().flatten().map(|value| i128::from(*value));
            values.next().map(|first| first + values.sum::<i128>())
        })
        .collect()
}

fn series(body: &mut String, values: &[Option<i128>], ceiling: i128, class: &str) {
    let mut points = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if let Some(value) = value {
            let x = 30 + index * 600 / values.len().saturating_sub(1).max(1);
            let y = 88 - *value * 70 / ceiling;
            points.push((x, y));
        } else {
            segment(body, &points, class);
            points.clear();
        }
    }
    segment(body, &points, class);
}

fn segment(body: &mut String, points: &[(usize, i128)], class: &str) {
    match points {
        [] => {}
        [(x, y)] => {
            write!(
                body,
                "<circle class=\"{class}\" cx=\"{x}\" cy=\"{y}\" r=\"2\"/>"
            )
            .expect("String");
        }
        _ => {
            write!(body, "<polyline class=\"{class}\" points=\"").expect("String");
            for (x, y) in points {
                write!(body, "{x},{y} ").expect("String");
            }
            body.push_str("\"/>");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_hours_break_lines_and_saved_zero_is_drawn() {
        let mut body = String::new();
        series(
            &mut body,
            &[Some(5), Some(4), None, Some(0)],
            5,
            "test-series",
        );
        assert_eq!(body.matches("<polyline").count(), 1);
        assert_eq!(body.matches("<circle").count(), 1);
        assert!(body.contains("cx=\"630\" cy=\"88\""));
        assert!(!body.contains("430,"));
        body.clear();
        series(&mut body, &[None, None], 1, "test-series");
        assert!(body.is_empty());
    }
}
