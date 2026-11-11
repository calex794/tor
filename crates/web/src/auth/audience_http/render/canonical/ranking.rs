//! A full-scope descriptive top-20 from the C owner; never a ranking of one page.
use super::{filters, metrics, time};
use crate::pages::{escape_html as e, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::audience::canonical::{
    CanonicalDimension, CanonicalQuery, CanonicalRankReport,
};

pub(crate) fn page(query: &CanonicalQuery, report: Option<&CanonicalRankReport>) -> Html<String> {
    let mut body = String::from(
        r#"<section class="section"><div class="container audience-books-width"><p class="eyebrow">Администратору сайта</p><h1>Подтверждённое чтение по книгам</h1><p>Одно первое подтверждённое завершение главы на пару «аккаунт–книга». Это не завершение всей книги и не число посетителей.</p>"#,
    );
    body.push_str(&crate::flow_navigation::links(query.days));
    filters(&mut body, query, true);
    let kind = match query.dimension {
        CanonicalDimension::Books => "книги",
        CanonicalDimension::Genres => "жанры",
        CanonicalDimension::Tags => "теги",
    };
    write!(body, r#"<section class="book-section-card" data-testid="canonical-ranking"><h2>Топ-20: {kind} по подтверждённым завершениям</h2><p>Порядок по числу первых завершений в текущем периоде, от большего к меньшему. Рейтинг рассчитан по всем подходящим книгам с выбранными фильтрами. При равном числе пар сохраняется порядок по номеру книги, групп — по названию.</p><p>Меньше 10 пар в периоде — малая выборка. Это описательные числа, а не оценка качества книги или статистической значимости.</p></section>"#).expect("String");
    match report {
        None => body.push_str(r#"<p role="alert" data-testid="canonical-rank-unavailable">Рейтинг временно недоступен. Неизвестные числа не заменяются нулями.</p>"#),
        Some(report) => {
            write!(body, r#"<section class="book-section-card" data-testid="canonical-rank-totals"><h2>Подходящих книг: <span data-metric="books">{}</span></h2><p>Общие числа до выбора топ-20 и разбиения на пересекающиеся жанры и теги. Сумма показанных групп не является общим итогом.</p><p>UTC; начало включено, конец не включён. {} / {} / {}</p>"#,
                report.totals.books, time(&report.previous_from_utc), time(&report.current_from_utc), time(&report.measured_at_utc)).expect("String");
            metrics(&mut body, report.totals);
            body.push_str("</section>");
            if report.rows.is_empty() {
                body.push_str(r#"<p data-testid="canonical-rank-empty">Нет подходящих строк для рейтинга.</p>"#);
            }
            for (index, row) in report.rows.iter().enumerate() {
                let rank = index + 1;
                let label = e(row.label.as_deref().unwrap_or("Без метки"));
                write!(body, r#"<article class="book-section-card audience-book-row" data-testid="canonical-rank-row" data-rank="{rank}" data-book-id="{}"><h2>{rank}. "#,
                    row.book_id.map(|id| id.to_string()).unwrap_or_default()).expect("String");
                if let Some(id) = row.book_id {
                    write!(body, r#"<a href="/books/{id}">{label}</a>"#).expect("String");
                } else { body.push_str(&label); }
                write!(body, r#"</h2><p>Подходящих книг: <span data-metric="books">{}</span></p>"#, row.counts.books).expect("String");
                if row.small_current_sample {
                    body.push_str(r#"<p data-testid="canonical-rank-small-current">Малая выборка в текущем периоде: меньше 10 пар.</p>"#);
                }
                if row.small_previous_sample {
                    body.push_str(r#"<p data-testid="canonical-rank-small-previous">Малая выборка в предыдущем периоде: меньше 10 пар.</p>"#);
                }
                metrics(&mut body, row.counts);
                body.push_str("</article>");
            }
        }
    }
    body.push_str("<section class=\"book-section-card\"><h2>Границы рейтинга</h2><p>Источник — сохранившиеся записи чтения C с совпадающими подтверждениями. Следующие главы и повторные завершения не добавляют новую пару «аккаунт–книга». Нуль обозначает отсутствие таких сохранившихся записей, а не отсутствие всех читателей.</p><p>Учитываются доступные сейчас публичные книги без возрастных ограничений с текущими жанрами и тегами. Библиотеки и подписки отражают нынешнее состояние. Пересекающиеся группы нельзя складывать. Рейтинг не меняет порядок публичного каталога и рекомендаций.</p><p>Сравнение двух периодов доступно при 10 и более парах в каждом. Показы и причинный эффект продвижения этим рейтингом не измеряются.</p></section></div></section>");
    layout("Топ-20 по подтверждённому чтению", "Описательный рейтинг первых завершений глав по книгам и группам", &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_reading::services::audience::canonical::{CanonicalCounts, CanonicalRankRow};

    #[test]
    fn rank_keeps_order_caveats_escaping_and_unknown_source_distinct() {
        let q = CanonicalQuery::new(7, CanonicalDimension::Books);
        let missing = page(&q, None).0;
        assert!(missing.contains("canonical-rank-unavailable"));
        assert!(!missing.contains("data-metric="));
        let report = CanonicalRankReport {
            measured_at_utc: "2026-11-11T10:10:00Z".into(),
            current_from_utc: "2026-11-04T10:10:00Z".into(),
            previous_from_utc: "2026-10-28T10:10:00Z".into(),
            totals: CanonicalCounts { books: 25, current_pairs: 20, ..Default::default() },
            rows: vec![CanonicalRankRow {
                book_id: Some(99), label: Some("<script>hidden</script>".into()),
                counts: CanonicalCounts { books: 1, current_pairs: 9, ..Default::default() },
                small_current_sample: true, small_previous_sample: true,
            }],
        };
        let html = page(&q, Some(&report)).0;
        assert!(html.contains("data-rank=\"1\" data-book-id=\"99\""));
        assert!(html.contains("data-metric=\"books\">25</span>"));
        assert!(html.contains("canonical-rank-small-current"));
        assert!(html.contains("&lt;script&gt;hidden&lt;/script&gt;"));
        assert!(!html.contains("<script>hidden"));
        assert!(!html.contains("canonical-next"));
    }
}
