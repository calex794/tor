//! One server-side moderator-only history page for all Book decisions.
use super::{deny, error_code, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Response},
    routing::get,
    Router,
};
use tor_accounts::services::security::session_token_digest;

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/moderation/books/{id}/history", get(book_history))
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
