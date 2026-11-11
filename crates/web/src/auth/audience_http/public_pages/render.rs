use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_search::domain::public_pages::{PageSlot, PageState, PublicPageReport};

pub(super) fn page(report: Option<&PublicPageReport>) -> Html<String> {
    let mut body = format!(
        r#"<section class="section"><div class="container author-width"><p class="eyebrow">Администратору сайта</p><h1>Доступность публичных страниц</h1>{}<p>Сохранённая проверка небольшой текущей выборки: главная, каталог, одна публичная книга и одна бесплатная глава. Это не проверка всех адресов сайта.</p>"#,
        crate::flow_navigation::links(7)
    );
    if let Some(report) = report {
        write!(
            body,
            "<p>Время отчёта: {}. Последняя проверка: {}.</p>",
            escape_html(&report.measured_at),
            escape_html(
                report
                    .checked_at
                    .as_deref()
                    .unwrap_or("ещё не подтверждена")
            )
        )
        .expect("String");
        if report.lost_targets() > 0 {
            write!(body, r#"<p role="alert" data-testid="pages-loss">Подтверждено недоступных адресов текущей выборки: {}.</p>"#, report.lost_targets()).expect("String");
        } else if report.checked_targets() == report.expected_targets()
            && report.expected_targets() > 0
        {
            body.push_str(r#"<p data-testid="pages-current-healthy">В текущей проверяемой выборке нет подтверждённых потерь. Восстановление ранее проверенных URL не утверждается.</p>"#);
        } else {
            body.push_str(r#"<p data-testid="pages-unknown">Результат известен не для всей текущей выборки. Отсутствие потерь не подтверждено.</p>"#);
        }
    } else {
        body.push_str(r#"<p role="alert">Проверка временно недоступна. Состояние неизвестно; нулевые потери не подставляются.</p>"#);
    }
    for slot in PageSlot::ALL {
        let row = report.and_then(|report| report.rows.iter().find(|row| row.slot == slot));
        let state = row.map(|row| row.state).unwrap_or(PageState::Unknown);
        let (code, label) = match state {
            PageState::Healthy => ("healthy", "Доступна в последней проверке"),
            PageState::Lost => ("lost", "Подтверждена потеря доступности"),
            PageState::Unknown => ("unknown", "Неизвестно"),
            PageState::NoTarget => ("no_target", "Нет подходящего публичного адреса"),
        };
        let title = match slot {
            PageSlot::Home => "Главная",
            PageSlot::Catalog => "Каталог",
            PageSlot::Book => "Публичная книга",
            PageSlot::FreeChapter => "Бесплатная глава",
        };
        write!(body, r#"<section class="book-section-card" data-testid="public-page-state" data-slot="{}" data-state="{code}"><h2>{title}</h2><p><strong>{label}</strong></p>"#, slot.code()).expect("String");
        if let Some(row) = row {
            if let Some(path) = &row.path {
                write!(
                    body,
                    r#"<p><a href="{}">{}</a></p>"#,
                    escape_html(path),
                    escape_html(path)
                )
                .expect("String");
            }
            if let Some(status) = row.status {
                write!(
                    body,
                    "<p>Последний подтверждённый HTTP-статус: {status}.</p>"
                )
                .expect("String");
            }
            if let Some(at) = &row.last_success_at {
                write!(
                    body,
                    "<p>Последняя успешная проверка этого адреса: {}.</p>",
                    escape_html(at)
                )
                .expect("String");
            }
        }
        body.push_str("</section>");
    }
    body.push_str(r#"<section class="book-section-card"><h2>Как понимать результат</h2>
<p>Потеря фиксируется только после успешного ответа HTTP 200 того же доступного публичного адреса. Первый неуспешный ответ, ошибка сети, смена адреса или версии и результат старше трёх минут означают «неизвестно».</p>
<p>Законное снятие книги или главы с публикации исключает адрес из текущей выборки. Оно не означает восстановление прежнего адреса. Закрытый платный контент не проверяется.</p>
<p>Фоновая проверка выполняется раз в минуту: не более двух одновременных HEAD-запросов, без читательских cookies и переходов по перенаправлениям. Открытие или обновление этого отчёта только читает сохранённый результат.</p></section>
<p><a class="button button--quiet" href="/admin/audience/pages">Обновить сохранённый результат</a></p></div></section>"#);
    layout(
        "Доступность публичных страниц",
        "Закрытый отчёт по текущей выборке публичных адресов.",
        &body,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_results_are_unknown_and_timestamps_are_escaped() {
        let missing = page(None).0;
        assert_eq!(
            missing
                .matches(r#"data-testid="public-page-state""#)
                .count(),
            4
        );
        assert!(!missing.contains("pages-current-healthy"));
        let report = PublicPageReport {
            measured_at: "<img src=x onerror=bad>".into(),
            checked_at: None,
            age_seconds: None,
            rows: Vec::new(),
        };
        let html = page(Some(&report)).0;
        assert!(html.contains("&lt;img"));
        assert!(!html.contains("<img src=x"));
        assert!(!html.contains("pages-current-healthy"));
    }
}
