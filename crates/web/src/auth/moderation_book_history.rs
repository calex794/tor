//! One server-side moderator-only history page for all Book decisions.
use super::{deny, error_code, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_accounts::{
    repositories::moderation_reviewed::{
        ReviewedCursor, ReviewedModerationDecision, ReviewedModerationPage,
    },
    services::security::session_token_digest,
};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/moderation/books/{id}/history", get(book_history))
        .route("/moderation/books/reviews", get(reviewed_index))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewedQuery {
    kind: Option<String>,
    outcome: Option<String>,
    before: Option<String>,
}

fn read_filter<'a>(value: Option<&'a str>, allowed: &[&str]) -> Result<Option<&'a str>, ()> {
    match value {
        None | Some("") | Some("all") => Ok(None),
        Some(item) if allowed.contains(&item) => Ok(Some(item)),
        _ => Err(()),
    }
}

fn render_reviewed(
    page: &ReviewedModerationPage,
    kind: Option<&str>,
    outcome: Option<&str>,
) -> String {
    let selected_kind = kind.unwrap_or("all");
    let selected_outcome = outcome.unwrap_or("all");
    let mut entries = String::new();
    for item in &page.items {
        let label = match item.cursor.kind.as_str() {
            "registration" => "Регистрация",
            "cover" => "Обложка",
            "deletion" => "Удаление",
            _ => "Неизвестный тип",
        };
        let result = if item.outcome == "approved" {
            "Одобрено"
        } else {
            "Отклонено"
        };
        let title = pages::escape_html(&item.book_title);
        let reviewer = pages::escape_html(&item.reviewer_name);
        let reason = pages::escape_html(&item.reason);
        entries.push_str(&format!(
            "<li><strong>{label}: {result}</strong> · \
             <a href=\"/moderation/books/{book_id}/history\">{title} (#{book_id})</a> · \
             Модератор {reviewer} (#{reviewer_id}) · \
             Время Unix {time} · Причина: {reason}</li>",
            book_id = item.cursor.book_id,
            reviewer_id = item.reviewer_user_id,
            time = item.cursor.decided_us / 1_000_000,
        ));
    }
    if entries.is_empty() {
        entries.push_str("<li>Рассмотренных заявок по фильтру нет.</li>");
    }
    let more = page.next_cursor.as_ref().map_or_else(String::new, |cursor| {
        format!(
            "<p><a href=\"/moderation/books/reviews?kind={selected_kind}&outcome={selected_outcome}&before={}\">\
             Следующие решения</a></p>",
            cursor.token()
        )
    });
    let kind_options = [
        ("all", "Все типы"),
        ("registration", "Регистрация"),
        ("cover", "Обложки"),
        ("deletion", "Удаление"),
    ]
    .into_iter()
    .map(|(value, label)| {
        let selected = if selected_kind == value { " selected" } else { "" };
        format!("<option value=\"{value}\"{selected}>{label}</option>")
    })
    .collect::<Vec<_>>()
    .join("");
    let outcome_options = [
        ("all", "Все решения"),
        ("approved", "Одобрено"),
        ("rejected", "Отклонено"),
    ]
    .into_iter()
    .map(|(value, label)| {
        let selected = if selected_outcome == value { " selected" } else { "" };
        format!("<option value=\"{value}\"{selected}>{label}</option>")
    })
    .collect::<Vec<_>>()
    .join("");
    pages::layout(
        "Рассмотренные заявки модерации",
        "Закрытая история решений по регистрации, обложкам и удалению.",
        &format!(
            "<section class=\"me-page\"><div class=\"container\">\
             <h1>Рассмотренные заявки</h1>\
             <p>Закрытый журнал решений модераторов сайта. \
             Это история, а не дополнительная очередь одобрения.</p>\
             <form method=\"get\" action=\"/moderation/books/reviews\">\
             <label for=\"reviewed-kind\">Тип заявки</label>\
             <select id=\"reviewed-kind\" name=\"kind\">{kind_options}</select>\
             <label for=\"reviewed-outcome\">Результат</label>\
             <select id=\"reviewed-outcome\" name=\"outcome\">{outcome_options}</select>\
             <button type=\"submit\">Применить фильтры</button></form>\
             <ol>{entries}</ol>{more}\
             <p><a href=\"/moderation/books/registrations\">Регистрация</a> · \
             <a href=\"/moderation/covers\">Обложки</a> · \
             <a href=\"/moderation/books/deletions\">Удаление</a></p>\
             </div></section>"
        ),
    )
    .0
}

pub(super) async fn reviewed_index(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<ReviewedQuery>,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let kind = match read_filter(query.kind.as_deref(), &["registration", "cover", "deletion"]) {
        Ok(kind) => kind,
        Err(()) => return deny(StatusCode::BAD_REQUEST),
    };
    let outcome = match read_filter(query.outcome.as_deref(), &["approved", "rejected"]) {
        Ok(outcome) => outcome,
        Err(()) => return deny(StatusCode::BAD_REQUEST),
    };
    let before = match query.before.as_deref().map(ReviewedCursor::parse).transpose() {
        Ok(value) => value,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let rows = match state
        .accounts
        .private_reviewed_moderation_decisions(&digest, kind, outcome, before.as_ref())
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(error_code(error)),
    };
    let mut response = private_response(Html(render_reviewed(&rows, kind, outcome)), &[]);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

fn render_book_history(
    book_id: i64,
    rows: &[tor_accounts::repositories::moderation_book_history::BookModerationHistoryEntry],
) -> String {
    let mut entries = String::new();
    for item in rows {
        let kind = match item.kind.as_str() {
            "registration" => "Регистрация",
            "cover" => "Обложка",
            "deletion" => "Удаление",
            _ => "Неизвестное решение",
        };
        let outcome = match item.outcome.as_str() {
            "approved" => "Одобрено",
            "rejected" => "Отклонено",
            _ => "Неизвестный результат",
        };
        let reviewer = pages::escape_html(&item.reviewer_name);
        let reason = pages::escape_html(&item.reason);
        let decision_ref = pages::escape_html(&item.decision_ref);
        entries.push_str(&format!(
            "<li><strong>{kind}: {outcome}</strong> · Модератор: \
             {reviewer} (#{reviewer_id}) · Время Unix: {timestamp} \
             · Заявка/решение #{decision_ref} · Причина: {reason}</li>",
            reviewer_id = item.reviewer_user_id,
            timestamp = item.decided_unix,
        ));
    }
    if entries.is_empty() {
        entries.push_str("<li>Подтверждённых решений пока нет.</li>");
    }
    pages::layout(
        "История решений модерации",
        "Сведения доступны только модераторам сайта и администраторам.",
        &format!(
            "<section class=\"me-page\"><div class=\"container\">\
             <h1>Системная история книги #{book_id}</h1>\
             <p>Регистрация, первичная и заменённые обложки, удаление. \
             История не влияет на права чтения и покупки.</p>\
             <ol>{entries}</ol>\
             <p><a href=\"/moderation/books/registrations\">Регистрация</a> \
             · <a href=\"/moderation/covers\">Обложки</a> \
             · <a href=\"/moderation/books/deletions\">Удаление</a></p>\
             </div></section>"
        ),
    )
    .0
}

pub(super) async fn book_history(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let rows = match state
        .accounts
        .private_book_moderation_history(&digest, id)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(error_code(error)),
    };
    let mut response = private_response(Html(render_book_history(id, &rows)), &[]);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

#[cfg(test)]
mod moderation_history_tests {
    use super::*;
    use tor_accounts::repositories::moderation_book_history::BookModerationHistoryEntry;

    #[test]
    fn reviewed_list_keeps_reviewers_private_and_escapes_untrusted_content() {
        let cursor = ReviewedCursor {
            decided_us: 1_800_000_000_000_000,
            kind: "cover".into(),
            book_id: 31,
            decision_id: 91,
        };
        let page = ReviewedModerationPage {
            items: vec![ReviewedModerationDecision {
                cursor: cursor.clone(),
                book_title: "<script>alert(1)</script>".into(),
                outcome: "rejected".into(),
                reviewer_user_id: 17,
                reviewer_name: "<img src=x>".into(),
                reason: "<svg/onload=alert(1)>".into(),
            }],
            next_cursor: Some(cursor),
        };
        let html = render_reviewed(&page, Some("cover"), Some("rejected"));
        assert!(html.contains("Обложка: Отклонено"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;img src=x&gt;"));
        assert!(html.contains("&lt;svg/onload=alert(1)&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("kind=cover&outcome=rejected&before=1800000000000000.cover.31.91"));
        assert_eq!(read_filter(Some("other"), &["approved", "rejected"]), Err(()));
    }

    #[test]
    fn internal_history_escapes_reviewer_name_and_reason() {
        let html = render_book_history(
            83,
            &[
                BookModerationHistoryEntry {
                    kind: "registration".into(),
                    outcome: "approved".into(),
                    reviewer_user_id: 17,
                    reviewer_name: "<script>moderator</script>".into(),
                    reason: "<img src=x>".into(),
                    decided_unix: 1_800_000_000,
                    decision_ref: "31".into(),
                },
                BookModerationHistoryEntry {
                    kind: "deletion".into(),
                    outcome: "rejected".into(),
                    reviewer_user_id: 18,
                    reviewer_name: "Other moderator".into(),
                    reason: "Недостаточно оснований".into(),
                    decided_unix: 1_800_000_001,
                    decision_ref: "2".into(),
                },
            ],
        );
        assert!(html.contains("Системная история книги #83"));
        assert!(html.contains("Регистрация: Одобрено"));
        assert!(html.contains("Удаление: Отклонено"));
        assert!(html.contains("&lt;script&gt;moderator&lt;/script&gt;"));
        assert!(html.contains("&lt;img src=x&gt;"));
        assert!(!html.contains("<script>moderator</script>"));
        assert!(!html.contains("<img src=x>"));
        assert!(html.contains("Регистрация"));
        assert!(html.contains("Обложки"));
    }
}
