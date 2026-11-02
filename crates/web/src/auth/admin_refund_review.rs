//! S036: site-administrator-only, audited and read-only refund review.
//! Never expose an approve/reject operation until the separate step-up and
//! approved commercial refund policy requirements are implemented.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Response, IntoResponse},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_commerce::repositories::refund_admin_review::{
    AdminRefundRequest, PgRefundAdminReview, RefundAdminViewError,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    before: Option<i64>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/admin/finance/refunds", get(page))
}

fn status(state: &str) -> &'static str {
    match state {
        "pending" => "Ожидает решения",
        "processing" => "В обработке",
        "approved" => "Внутренний возврат одобрен",
        "rejected" => "Отклонена",
        _ => "Неизвестный статус",
    }
}

fn render_request(row: &AdminRefundRequest) -> String {
    let policy = row.policy_version.as_deref().unwrap_or("Не зафиксирована");
    let deadline = row.deadline_utc.as_deref().unwrap_or("Не зафиксирован");
    format!(
        r#"<article class="me-now-card">
        <h2>Заявка №{} — {}</h2>
        <p>Покупка №{} · Читатель №{}: {}</p>
        <p>Книга: {} · Глава: {}</p>
        <p>Сумма: {} коп. · Версия решения: {}</p>
        <p>Поступила (UTC): {}</p>
        <p>Политика при покупке: {} · Дедлайн (UTC): {}</p>
        <p>Первый защищённый доступ: {} · Отзыв доступа: {}</p>
        <p>Причина читателя: {}</p></article>"#,
        row.id,
        status(&row.state),
        row.purchase_id,
        row.buyer_user_id,
        pages::escape_html(&row.buyer_name),
        pages::escape_html(&row.book_title),
        pages::escape_html(&row.chapter_title),
        row.amount_minor,
        row.revision,
        pages::escape_html(&row.created_utc),
        pages::escape_html(policy),
        pages::escape_html(deadline),
        if row.first_exposed { "Да" } else { "Нет" },
        if row.revoked { "Да" } else { "Нет" },
        pages::escape_html(&row.reason),
    )
}

fn page_shell(content: &str) -> Response {
    let mut response = private_response(
        pages::layout(
            "Заявки на возврат — администратор",
            "Аудируемый просмотр заявок на возврат.",
            &format!(
                r#"<section class="me-page"><div class="container">
                <header class="ui-page-header"><h1>Заявки на возврат покупок</h1></header>
                <p>Только просмотр. Статус заявки не является разрешением
                на возврат средств. Перед любым решением условия покупки,
                факт выдачи доступа и срок должны проверяться повторно.</p>
                {content}<p><a href="/me">Кабинет</a></p>
                </div></section>"#
            ),
        ),
        &[],
    ).into_response();
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Response {
    match session_user(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    }
    let Some(session_token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&session_token);
    let requests = match PgRefundAdminReview::new(state.accounts.pool().clone())
        .recent(&digest, query.before)
        .await
    {
        Ok(rows) => rows,
        Err(RefundAdminViewError::NotFound) => return deny(StatusCode::NOT_FOUND),
        Err(RefundAdminViewError::InvalidCursor) => {
            return deny(StatusCode::BAD_REQUEST);
        }
        Err(RefundAdminViewError::Unavailable) => {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    let mut body = String::new();
    if requests.is_empty() {
        body.push_str("<p>Заявок не найдено.</p>");
    }
    for request in &requests {
        body.push_str(&render_request(request));
    }
    if requests.len() == 30 {
        if let Some(last) = requests.last() {
            write!(
                body,
                r#"<p><a href="/admin/finance/refunds?before={}">Предыдущие заявки</a></p>"#,
                last.id
            ).expect("html cursor");
        }
    }
    page_shell(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s036_admin_review_escapes_all_user_supplied_text_and_has_no_decisions() {
        let row = AdminRefundRequest {
            id: 13,
            purchase_id: 9,
            buyer_user_id: 6,
            buyer_name: "<script>buyer</script>".to_owned(),
            book_title: "<img src=x>".to_owned(),
            chapter_title: "<b>Chapter</b>".to_owned(),
            amount_minor: 300,
            reason: "<script>alert(1)</script>".to_owned(),
            state: "pending".to_owned(),
            revision: 1,
            policy_version: Some("policy-x".to_owned()),
            deadline_utc: None,
            created_utc: "2026-11-03 00:00".to_owned(),
            first_exposed: false,
            revoked: false,
        };
        let html = render_request(&row);
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;img src=x&gt;"));
        assert!(html.contains("&lt;b&gt;Chapter&lt;/b&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<form"));
        assert!(!html.contains("name=\"decision\""));
        assert!(html.contains("Ожидает решения"));
    }
}
