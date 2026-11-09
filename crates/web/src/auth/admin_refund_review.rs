//! S036: site-administrator-only, audited and read-only refund review.
//! Never expose an approve/reject operation until the separate step-up and
//! approved commercial refund policy requirements are implemented.
mod approval;
mod prepare;
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_commerce::repositories::admin_support_refunds::{
    PgSupportRefunds, RefundPurchase, SupportRefundError,
};
use tor_commerce::repositories::refund_admin_review::{PgRefundAdminReview, RefundAdminViewError};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    before: Option<i64>,
    q: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/admin/finance/refunds", get(page))
        .route("/admin/finance/refunds/user/{id}", get(buyer))
        .route(
            "/admin/finance/refunds/prepare",
            post(prepare::submit).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/admin/finance/refunds/approve",
            post(approval::submit).layer(DefaultBodyLimit::max(4096)),
        )
}

#[cfg(test)]
use tor_commerce::repositories::refund_admin_review::AdminRefundRequest;

#[cfg(test)]
fn status(state: &str) -> &'static str {
    match state {
        "pending" => "Ожидает решения",
        "processing" => "В обработке",
        "approved" => "Внутренний возврат одобрен",
        "rejected" => "Отклонена",
        _ => "Неизвестный статус",
    }
}

#[cfg(test)]
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

fn page_shell(content: &str, cookies: &[String]) -> Response {
    let mut response = private_response(
        pages::layout(
            "Возвраты — администратор",
            "Административное рассмотрение конкретных покупок.",
            &format!(
                r#"<section class="me-page"><div class="container">
                <header class="ui-page-header"><h1>Возвраты покупок</h1></header>
                <p>Доступно только действующему администратору.
                Выбирайте точный идентификатор покупки, а не произвольную сумму.
                Внутренний возврат в кошелёк не является возвратом через банк
                или платёжного провайдера.</p>
                {content}<p><a href="/me">Кабинет</a></p>
                </div></section>"#
            ),
        ),
        cookies,
    )
    .into_response();
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

fn response_error(err: SupportRefundError) -> Response {
    deny(match err {
        SupportRefundError::Invalid => StatusCode::BAD_REQUEST,
        SupportRefundError::Forbidden => StatusCode::FORBIDDEN,
        SupportRefundError::NotFound => StatusCode::NOT_FOUND,
        SupportRefundError::Conflict => StatusCode::CONFLICT,
        SupportRefundError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    })
}

fn session_digest_from(state: &AuthState, headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(session_token_digest(&token))
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Response {
    if !matches!(session_user(&state, &headers).await, Ok(Some(_))) {
        return deny(StatusCode::UNAUTHORIZED);
    }
    let digest = match session_digest_from(&state, &headers) {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    let mut body = String::from(
        "<form method=\"get\" action=\"/admin/finance/refunds\" class=\"auth-form\">
         <label for=\"refund-user-search\">Пользователь: имя, @handle или ID</label>
         <input id=\"refund-user-search\" name=\"q\" maxlength=\"80\" required>
         <button type=\"submit\">Найти</button></form>
         <p><a href=\"/admin/support\">Обращения в поддержку</a></p>",
    );
    if let Some(term) = query.q.as_deref() {
        match PgSupportRefunds::new(state.accounts.pool().clone())
            .search(&digest, term)
            .await
        {
            Ok(users) => {
                if users.is_empty() {
                    body.push_str("<p>Совпадений нет.</p>");
                }
                for user in users {
                    let handle = user.handle.unwrap_or_default();
                    write!(
                        body,
                        "<p><a href=\"/admin/finance/refunds/user/{}\">{}
                         (@{}) · ID {}</a></p>",
                        user.id,
                        pages::escape_html(&user.name),
                        pages::escape_html(&handle),
                        user.id
                    )
                    .expect("render user");
                }
            }
            Err(error) => return response_error(error),
        }
    } else {
        // Even the empty admin page must revalidate live authority.
        match PgRefundAdminReview::new(state.accounts.pool().clone())
            .recent(&digest, query.before)
            .await
        {
            Ok(_) => {}
            Err(RefundAdminViewError::NotFound) => return deny(StatusCode::FORBIDDEN),
            Err(RefundAdminViewError::InvalidCursor) => return deny(StatusCode::BAD_REQUEST),
            Err(RefundAdminViewError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    }
    page_shell(&body, &[])
}

fn render_purchase(
    p: &RefundPurchase,
    buyer_id: i64,
    support: Option<i64>,
    csrf: &str,
    enabled: bool,
) -> String {
    let mut body = format!(
        "<article class=\"me-now-card\"><h2>Покупка №{} — {} / {}</h2>
         <p>Стоимость: {}.{:02} ₽ · {} UTC</p>
         <p>Доступ к главе: {} · Возврат/отзыв: {}</p>",
        p.id,
        pages::escape_html(&p.book),
        pages::escape_html(&p.chapter),
        p.amount_minor / 100,
        p.amount_minor % 100,
        pages::escape_html(&p.acquired),
        if p.first_exposed {
            "выдавался"
        } else {
            "ещё не выдавался"
        },
        if p.revoked { "да" } else { "нет" },
    );
    if let Some(position) = p.batch_position {
        write!(
            body,
            "<p>Позиция {} в пакетной покупке (можно вернуть отдельно).</p>",
            position
        )
        .expect("batch label");
    }
    if let Some(status) = &p.request_state {
        write!(
            body,
            "<p>Последний запрос: {}</p>",
            pages::escape_html(status)
        )
        .expect("refund state");
    }
    if enabled && !p.revoked {
        match (
            p.request_id,
            p.request_state.as_deref(),
            p.request_revision,
            p.support_intent,
        ) {
            (Some(request), Some("pending"), Some(revision), true) => {
                body.push_str(&approval::support_form(request, revision, csrf));
            }
            (None, _, _, _) | (_, Some("rejected"), _, _) => {
                if let Some(conversation) = support {
                    write!(
                        body,
                        "<form method=\"post\" action=\"/admin/finance/refunds/prepare\" autocomplete=\"off\">
                         <input type=\"hidden\" name=\"buyer_id\" value=\"{}\">
                         <input type=\"hidden\" name=\"purchase_id\" value=\"{}\">
                         <input type=\"hidden\" name=\"conversation_id\" value=\"{}\">
                         <input type=\"hidden\" name=\"command_key\" value=\"{}\">
                         <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
                         <label>Основание исключительного возврата через поддержку
                           <textarea name=\"reason\" maxlength=\"1000\" required></textarea>
                         </label>
                         <button type=\"submit\">Подготовить возврат этой покупки</button></form>",
                        buyer_id, p.id, conversation,
                        tor_accounts::services::security::generate_session_token(),
                        pages::escape_html(csrf),
                    ).expect("support refund form");
                }
            }
            _ => {}
        }
    }
    body.push_str("</article>");
    body
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BuyerCursor {
    before: Option<i64>,
}

pub(super) async fn buyer(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    Query(cursor): Query<BuyerCursor>,
) -> Response {
    if !matches!(session_user(&state, &headers).await, Ok(Some(_))) {
        return deny(StatusCode::UNAUTHORIZED);
    }
    let digest = match session_digest_from(&state, &headers) {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let detail = match PgSupportRefunds::new(state.accounts.pool().clone())
        .user_purchases(&digest, user_id, cursor.before)
        .await
    {
        Ok(Some(detail)) => detail,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(err) => return response_error(err),
    };
    let enabled = approval::enabled(&state);
    let (csrf, cookies) = if enabled {
        state.cookies.csrf_for_page(&headers)
    } else {
        (String::new(), Vec::new())
    };
    let mut body = format!(
        "<h2>Профиль: {} · ID {}</h2><p>Имя пользователя: @{}</p>
         <p><a href=\"/admin/finance/refunds\">Вернуться к поиску</a></p>",
        pages::escape_html(&detail.user.name),
        detail.user.id,
        pages::escape_html(detail.user.handle.as_deref().unwrap_or("не задано")),
    );
    if let Some(case_id) = detail.support_conversation_id {
        write!(
            body,
            "<p><a href=\"/admin/support/{}\">Обращение в поддержку №{}</a></p>",
            case_id, case_id
        )
        .expect("case link");
    } else {
        body.push_str(
            "<p>Пользователь ещё не оставил подтверждённого обращения.
                       Возврат через эту панель невозможен.</p>",
        );
    }
    if detail.purchases.is_empty() {
        body.push_str("<p>Покупок глав нет.</p>");
    }
    let next = detail.purchases.last().map(|p| p.id);
    let count = detail.purchases.len();
    for purchase in &detail.purchases {
        body.push_str(&render_purchase(
            purchase,
            user_id,
            detail.support_conversation_id,
            &csrf,
            enabled,
        ));
    }
    if count == 30 {
        if let Some(last) = next {
            write!(
                body,
                "<p><a href=\"/admin/finance/refunds/user/{}?before={}\">Следующие покупки</a></p>",
                user_id, last
            )
            .expect("purchase cursor");
        }
    }
    page_shell(&body, &cookies)
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
