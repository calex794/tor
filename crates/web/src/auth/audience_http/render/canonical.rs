//! C-owned retained facts; aggregate metadata only, never a reader history.
use super::{evidence::encode, time};
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::audience::canonical::{
    CanonicalCounts, CanonicalQuery, CanonicalReport,
};

pub(crate) fn page(query: &CanonicalQuery, report: Option<&CanonicalReport>) -> Html<String> {
    let mut body = String::from(
        r#"<section class="section"><div class="container audience-books-width"><p class="eyebrow">Администратору сайта</p><h1>Подтверждённое чтение по книгам</h1><p>Одно первое подтверждённое завершение главы на пару «аккаунт–книга». Это не завершение всей книги и не число посетителей.</p>"#,
    );
    write!(
        body,
        r#"<p><a href="/admin/audience?days={}">Вернуться в «Аудиторию»</a></p>"#,
        query.days
    )
    .expect("String");
    filters(&mut body, query);
    match report {
        None=>body.push_str(r#"<p role="alert" data-testid="canonical-unavailable">Данные временно недоступны. Нули не подставляются.</p>"#),
        Some(report)=>{
            write!(body,r#"<section class="book-section-card" data-testid="canonical-totals"><h2>Подходящих книг: <span data-metric="books">{}</span></h2><p>Числа по всем подходящим книгам, до пагинации. Пересекающиеся жанры и теги не умножают общий итог.</p><p>Расчёт в UTC; два соседних периода. Начало включено, конец не включён.</p><p>{} / {} / {}</p>"#,report.totals.books,time(&report.previous_from_utc),time(&report.current_from_utc),time(&report.measured_at_utc)).expect("String");
            metrics(&mut body,report.totals); body.push_str("</section>");
            if report.rows.is_empty(){body.push_str(r#"<p data-testid="canonical-empty">Нет строк для этой страницы и фильтров.</p>"#);}
            for row in &report.rows {
                let label=escape_html(row.label.as_deref().unwrap_or("Без метки"));
                write!(body,r#"<article class="book-section-card audience-book-row" data-testid="canonical-row" data-book-id="{}"><h2>"#,row.book_id.map(|v|v.to_string()).unwrap_or_default()).expect("String");
                if let Some(id)=row.book_id {write!(body,r#"<a href="/books/{id}">{label}</a>"#).expect("String");} else {body.push_str(&label);}
                write!(body,r#"</h2><p>Подходящих книг: <span data-metric="books">{}</span></p>"#,row.counts.books).expect("String");
                metrics(&mut body,row.counts);body.push_str("</article>");
            }
            body.push_str(r#"<nav class="audience-books-pagination">"#);
            if query.after_id.is_some()||query.after_label.is_some(){write!(body,r#"<a class="button button--quiet" data-testid="canonical-first" href="{}">В начало</a>"#,escape_html(&url(query,None,None))).expect("String");}
            if report.next_after_id.is_some()||report.next_after_label.is_some(){write!(body,r#"<a class="button" data-testid="canonical-next" href="{}">Следующие</a>"#,escape_html(&url(query,report.next_after_id,report.next_after_label.as_deref()))).expect("String");}
            body.push_str("</nav>");
        }
    }
    body.push_str("<section class=\"book-section-card\"><p>Источник: сохранившиеся записи чтения C и их совпадающие подтверждения. Повторы и последующие главы не создают новую пару. Нуль означает ноль таких записей, а не отсутствие всех читателей.</p><p>Только доступные сейчас публичные книги без возрастных ограничений, с нынешними жанрами и тегами. Скрытая книга исключается вместе с числами.</p><p>Библиотеки и подписки — текущее состояние, не история действий. Показы, конверсия и возвраты здесь не измеряются. По нулю чтений нельзя заключить, что книгу не показывают.</p><p>Сравнение доступно при наличии не менее 10 пар в каждом периоде; это описательная разница, не статистическая значимость. По 20 строк в стабильном порядке, не рейтинг.</p></section></div></section>");
    layout("Подтверждённое чтение по книгам","Одно первое подтверждённое завершение главы на пару «аккаунт–книга». Это не завершение всей книги и не число посетителей.",&body)
}
fn metrics(body: &mut String, c: CanonicalCounts) {
    body.push_str("<dl>");
    for (key, label, value) in [
        (
            "current",
            "Первые завершения глав в книге: текущий период",
            c.current_pairs,
        ),
        ("previous", "Предыдущий период", c.previous_pairs),
        (
            "library",
            "Записей книг в библиотеках сейчас",
            c.library_links,
        ),
        ("follows", "Подписок на книги сейчас", c.follow_links),
    ] {
        write!(
            body,
            r#"<dt>{label}</dt><dd data-metric="{key}">{value}</dd>"#
        )
        .expect("String");
    }
    body.push_str("</dl>");
    match c.comparison_delta() {
        Some(delta) => write!(
            body,
            r#"<p data-testid="canonical-delta">Изменение числа пар «аккаунт–книга»: {delta:+}</p>"#
        )
        .expect("String"),
        None => body.push_str(
            r#"<p data-testid="canonical-small">Малая выборка: сравнение не вычисляется.</p>"#,
        ),
    }
}
fn filters(body: &mut String, q: &CanonicalQuery) {
    body.push_str(r#"<form class="author-form audience-books-filters" method="get" action="/admin/audience/reading"><div><label for="canonical-days">Дней в периоде</label><select id="canonical-days" name="days">"#);
    for days in [1, 7, 30, 90] {
        write!(
            body,
            r#"<option value="{days}"{}>{days}</option>"#,
            if days == q.days { " selected" } else { "" }
        )
        .expect("String");
    }
    body.push_str(r#"</select></div><div><label for="canonical-kind">Группировать по</label><select id="canonical-kind" name="kind">"#);
    for (key, label) in [("books", "Книги"), ("genres", "Жанры"), ("tags", "Теги")] {
        write!(
            body,
            r#"<option value="{key}"{}>{label}</option>"#,
            if key == q.dimension.key() {
                " selected"
            } else {
                ""
            }
        )
        .expect("String");
    }
    body.push_str("</select></div>");
    for (key, label, value, missing) in [
        ("genre", "Жанр", &q.genre, q.without_genre),
        ("tag", "Тег", &q.tag, q.without_tag),
    ] {
        write!(body,r#"<div><label for="canonical-{key}">{label}</label><input id="canonical-{key}" name="{key}" maxlength="40" value="{}"><label class="audience-missing-label"><input type="checkbox" name="without_{key}" value="true"{}><span>Без метки</span></label></div>"#,escape_html(value.as_deref().unwrap_or("")),if missing{" checked"}else{""}).expect("String");
    }
    write!(body,r#"<p><button class="button" type="submit">Показать</button> <a href="/admin/audience/reading?days={}">Сбросить фильтры</a></p></form>"#,q.days).expect("String");
}
fn url(q: &CanonicalQuery, id: Option<i64>, label: Option<&str>) -> String {
    let mut url = format!(
        "/admin/audience/reading?days={}&kind={}",
        q.days,
        q.dimension.key()
    );
    for (key, value) in [
        ("genre", q.genre.as_deref()),
        ("tag", q.tag.as_deref()),
        ("after_label", label),
    ] {
        if let Some(value) = value {
            write!(url, "&{key}={}", encode(value)).expect("String");
        }
    }
    for (key, value) in [
        ("without_genre", q.without_genre),
        ("without_tag", q.without_tag),
    ] {
        if value {
            write!(url, "&{key}=true").expect("String");
        }
    }
    if let Some(id) = id {
        write!(url, "&after_id={id}").expect("String");
    }
    url
}
#[cfg(test)]
mod tests {
    use super::*;
    use tor_reading::services::audience::canonical::{CanonicalDimension, CanonicalRow};
    #[test]
    fn source_error_is_not_zero_and_metadata_is_escaped() {
        let q = CanonicalQuery::new(7, CanonicalDimension::Tags);
        let failed = page(&q, None).0;
        assert!(failed.contains("canonical-unavailable"));
        assert!(!failed.contains("data-metric="));
        let report = CanonicalReport {
            measured_at_utc: "2026-11-11T00:00:00Z".into(),
            current_from_utc: "2026-11-04T00:00:00Z".into(),
            previous_from_utc: "2026-10-28T00:00:00Z".into(),
            totals: CanonicalCounts::default(),
            rows: vec![CanonicalRow {
                book_id: None,
                label: Some("<script>alert(1)</script>".into()),
                counts: CanonicalCounts::default(),
            }],
            next_after_id: None,
            next_after_label: Some("x & y".into()),
        };
        let html = page(&q, Some(&report)).0;
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("canonical-small"));
        assert!(!html.contains("data-testid=\"canonical-delta\""));
        assert!(html.contains("after_label=x%20%26%20y"));
    }
}
