//! RET-R03: shared read-only continuation demand tab for owners and site admins.
//! Counts come only from verified requests; no reader identities or editorial states.
use super::{AuthState, deny};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_reading::repositories::demand::{
    ContinuationStatsReport, PgContinuationDemand,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StatsQuery {
    after_waiting: Option<i64>,
    after_book: Option<i64>,
}

impl StatsQuery {
    fn cursor(self) -> Result<Option<(i64, i64)>, StatusCode> {
        match (self.after_waiting, self.after_book) {
            (None, None) => Ok(None),
            (Some(waiting), Some(book)) if waiting >= 0 && book > 0 => {
                Ok(Some((waiting, book)))
            }
            _ => Err(StatusCode::BAD_REQUEST),
        }
    }
}

pub(super) async fn owner(
    State(state): State<AuthState>,
    headers: HeaderMap,
    query: Result<Query<StatsQuery>, QueryRejection>,
) -> Response {
    report_page(state, headers, query, false).await
}

pub(super) async fn administrator(
    State(state): State<AuthState>,
    headers: HeaderMap,
    query: Result<Query<StatsQuery>, QueryRejection>,
) -> Response {
    report_page(state, headers, query, true).await
}

async fn report_page(
    state: AuthState,
    headers: HeaderMap,
    query: Result<Query<StatsQuery>, QueryRejection>,
    sitewide: bool,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return if sitewide {
            private_response(StatusCode::UNAUTHORIZED, &[])
        } else {
            private_response(Redirect::to("/login"), &[])
        };
    };
    // Always check current administrator status, not a cached owner flag.
    let actor = match state.accounts.session_actor(&session_token_digest(&token)).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if sitewide && !actor.administrator {
        return deny(StatusCode::FORBIDDEN);
    }
    let cursor = match query {
        Ok(Query(query)) => match query.cursor() {
            Ok(cursor) => cursor,
            Err(status) => return deny(status),
        },
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let owner_id = if sitewide {
        None
    } else {
        Some(actor.actor.user_id.get())
    };
    let report = match PgContinuationDemand::new(state.accounts.pool().clone())
        .stats_report(owner_id, cursor)
        .await
    {
        Ok(report) => report,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(render(&report, &csrf, sitewide), &cookies)
}

fn render(report: &ContinuationStatsReport, csrf: &str, sitewide: bool) -> axum::response::Html<String> {
    let base = if sitewide {
        "/admin/audience/continuations"
    } else {
        "/me/stats/continuations"
    };
    let mut body = format!(
        "<section class=\"me-page\"><div class=\"container{}\">",
        if sitewide { "" } else { " account-layout" }
    );
    if !sitewide {
        body.push_str(&pages::account_navigation("/me/stats/continuations", csrf));
        body.push_str("<div class=\"account-main\">");
    }
    body.push_str(if sitewide {
        "<p class=\"eyebrow\">Администратор · статистика сайта</p>\
         <h1>Статистика — Продолжения</h1>\
         <nav aria-label=\"Вкладки статистики\">\
         <a href=\"/admin/audience\">Общая статистика</a> · \
         <a href=\"/admin/audience/books\">Книги</a> · \
         <a href=\"/admin/audience/continuations\" aria-current=\"page\">Продолжения</a>\
         </nav>"
    } else {
        "<p class=\"eyebrow\">Кабинет владельца</p><h1>Статистика — Продолжения</h1>\
         <nav aria-label=\"Вкладки статистики\">\
         <a href=\"/me/books\">Мои книги</a> · \
         <a href=\"/me/stats/continuations\" aria-current=\"page\">Продолжения</a>\
         </nav>"
    });
    let _ = write!(
        body,
        "<section aria-label=\"Сводка запросов\">\
         <p>Ждут продолжения: <strong data-metric=\"waiting\">{}</strong> · \
         Книг с запросами: <strong data-metric=\"waiting-books\">{}</strong> · \
         Новых запросов за 7 дней: <strong data-metric=\"new-week\">{}</strong></p>\
         <p>Всего зарегистрированных запросов за всё время: <strong data-metric=\"historical\">{}</strong>. \
         Книг в области отчёта: {}.</p></section>",
        report.total_waiting, report.waiting_books, report.new_seven_days,
        report.historical, report.total_books,
    );
    body.push_str(
        "<p>«Ждут» — подтверждённые запросы после последней доступной главы. \
         После выхода новой главы текущий счётчик начинается заново, история сохраняется. \
         Повторное нажатие одного читателя на одной главе не увеличивает число.</p>\
         <table><thead><tr><th>Книга</th>");
    if sitewide {
        body.push_str("<th>Владелец</th>");
    }
    body.push_str(
        "<th>Последняя глава</th><th>Ждут</th><th>Новые за 7 дней</th>\
         <th>Все запросы</th></tr></thead><tbody>",
    );
    for book in &report.books {
        let _ = write!(
            body,
            "<tr><td><a href=\"/me/books/{}\">{}</a></td>",
            book.book_id,
            pages::escape_html(&book.title),
        );
        if sitewide {
            let _ = write!(
                body,
                "<td>{}</td>",
                pages::escape_html(&book.owner_name),
            );
        }
        let chapter = book.latest_chapter_no.map_or_else(
            || "—".to_owned(),
            |number| number.to_string(),
        );
        let _ = write!(
            body,
            "<td>{chapter}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            book.waiting, book.new_seven_days, book.historical
        );
    }
    body.push_str("</tbody></table>");
    if report.books.is_empty() {
        body.push_str("<p>В выбранной области пока нет книг или запросов на этой странице.</p>");
    }
    if let Some((waiting, book)) = report.next_after {
        let _ = write!(
            body,
            "<p><a href=\"{base}?after_waiting={waiting}&amp;after_book={book}\">Следующие книги</a></p>"
        );
    }
    if sitewide {
        body.push_str("<p><a href=\"/admin/audience\">Вернуться к общей статистике</a></p>");
    }
    body.push_str(if sitewide { "</div></section>" } else { "</div></div></section>" });
    pages::private_account_layout("Продолжения — статистика", "Подтверждённые запросы читателей.", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_requires_full_positive_key_and_rejects_extras() {
        assert_eq!(StatsQuery::default().cursor(), Ok(None));
        assert_eq!(StatsQuery { after_waiting: Some(0), after_book: Some(7) }.cursor(), Ok(Some((0, 7))));
        assert!(StatsQuery { after_waiting: Some(-1), after_book: Some(7) }.cursor().is_err());
        assert!(StatsQuery { after_waiting: Some(1), after_book: None }.cursor().is_err());
    }

    #[test]
    fn html_escapes_hostile_titles_and_hides_reader_identifiers() {
        let report = ContinuationStatsReport {
            books: vec![tor_reading::repositories::demand::ContinuationStatsBook {
                book_id: 3,
                title: "<script>bad()</script>".into(),
                owner_user_id: 1,
                owner_name: "<script>owner</script>".into(),
                latest_chapter_no: Some(51),
                waiting: 4,
                new_seven_days: 2,
                historical: 8,
            }],
            total_books: 1,
            waiting_books: 1,
            total_waiting: 4,
            new_seven_days: 2,
            historical: 8,
            next_after: None,
        };
        let html = render(&report, "csrf", true).0;
        assert!(html.contains("data-metric=\"waiting\">4"));
        assert!(html.contains("Продолжения"));
        assert!(html.contains("&lt;script&gt;bad()&lt;/script&gt;"));
        assert!(html.contains("&lt;script&gt;owner&lt;/script&gt;"));
        assert!(!html.contains("<script>bad()"));
        assert!(!html.contains("reader_id"));
    }
}
