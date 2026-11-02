//! S036: buyer-side D-06 requests. Never grant refunds or expose a
//! moderator decision here. Only the commerce transaction may accept an intent.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{DefaultBodyLimit, Form, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{
    domain::money::IdempotencyKey,
    repositories::{
        refund_buyer_view::{BuyerRefundPurchase, PgRefundBuyerView},
        refund_preflight::{DirectRefundGate, PgDirectRefundPreflight},
        refund_requests::{DirectRefundRequestError, PgDirectRefundRequests},
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RefundForm {
    purchase_id: String,
    command_key: String,
    csrf_token: String,
    reason: String,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route(
        "/me/refunds",
        get(page).post(submit).layer(DefaultBodyLimit::max(4096)),
    )
}

fn enabled(state: &AuthState) -> bool {
    state.paid_checkout_enabled && state.refund_browser_enabled
}

fn noindex(mut response: Response) -> Response {
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

fn shell(content: &str) -> Html<String> {
    pages::layout(
        "Заявки на возврат",
        "История заявок на возврат купленных глав.",
        &format!(
            "<section class=\"section auth-section\"><div class=\"container\">
             <div class=\"auth-card\"><p class=\"eyebrow\">Кабинет читателя</p>
             <h1>Заявки на возврат</h1>{content}
             <p><a href=\"/me/purchases\">Мои купленные главы</a> ·
             <a href=\"/me\">Мой кабинет</a></p></div></div></section>"
        ),
    )
}

fn error(status: StatusCode, message: &str) -> Response {
    noindex(private_response(
        (
            status,
            shell(&format!(
                "<p role=\"alert\">{}</p>",
                pages::escape_html(message)
            )),
        ),
        &[],
    ))
}

fn positive_id(raw: &str) -> Option<i64> {
    if raw.is_empty() || raw.len() > 19 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok().filter(|id| *id > 0)
}

fn state_label(state: &str) -> &'static str {
    match state {
        "pending" => "На рассмотрении",
        "processing" => "Рассматривается",
        "approved" => "Одобрена: внутренний возврат завершён",
        "rejected" => "Отклонена",
        _ => "Состояние не определено",
    }
}

fn request_error(error: DirectRefundRequestError) -> (StatusCode, &'static str) {
    match error {
        DirectRefundRequestError::InvalidRequest => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Проверьте причину и идентификатор заявки.",
        ),
        DirectRefundRequestError::NotFound => (
            StatusCode::NOT_FOUND,
            "Покупка не найдена в вашем аккаунте.",
        ),
        DirectRefundRequestError::PolicyUnavailable
        | DirectRefundRequestError::FirstGrantBarrierUnavailable => (
            StatusCode::CONFLICT,
            "Для этой покупки нет утверждённых условий возврата или подтверждённого учёта чтения.",
        ),
        DirectRefundRequestError::WindowExpired
        | DirectRefundRequestError::AlreadyOpened
        | DirectRefundRequestError::Revoked
        | DirectRefundRequestError::AlreadyOpen
        | DirectRefundRequestError::UnverifiedPurchase => (
            StatusCode::CONFLICT,
            "Эта покупка сейчас не соответствует условиям подачи заявки.",
        ),
        DirectRefundRequestError::DuplicateKeyDifferentRequest => (
            StatusCode::CONFLICT,
            "Этот ключ уже применялся для другой заявки.",
        ),
        DirectRefundRequestError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Проверка заявки временно недоступна. Повторите попытку позже.",
        ),
    }
}

fn card(row: &BuyerRefundPurchase, csrf: &str, allowed: bool) -> String {
    let mut markup = format!(
        "<article class=\"library-card\"><h2>{}</h2><p>{}</p><p>Покупка №{} · {} коп.</p>",
        pages::escape_html(&row.book_title),
        pages::escape_html(&row.chapter_title),
        row.purchase_id,
        row.amount_minor,
    );
    if let (Some(id), Some(state)) = (row.request_id, row.request_state.as_deref()) {
        write!(
            markup,
            "<p role=\"status\">Заявка №{id}: {}</p>",
            state_label(state)
        )
        .expect("html");
    } else if allowed {
        let key = generate_session_token();
        write!(
            markup,
            "<form method=\"post\" action=\"/me/refunds\">
             <input type=\"hidden\" name=\"purchase_id\" value=\"{}\">
             <input type=\"hidden\" name=\"command_key\" value=\"{}\">
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
             <label for=\"reason-{}\">Причина возврата</label>
             <textarea id=\"reason-{}\" name=\"reason\" rows=\"3\" maxlength=\"2000\" required></textarea>
             <button class=\"button\" type=\"submit\">Подать заявку</button></form>",
            row.purchase_id,
            pages::escape_html(&key),
            pages::escape_html(csrf),
            row.purchase_id,
            row.purchase_id,
        )
        .expect("html");
    } else {
        markup.push_str("<p>Для этой покупки подача заявки сейчас недоступна.</p>");
    }
    markup.push_str("</article>");
    markup
}

pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !enabled(&state) {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return noindex(private_response(Redirect::to("/login"), &[])),
        Err(status) => return deny(status),
    };
    let pool = state.accounts.pool().clone();
    let rows = match PgRefundBuyerView::new(pool.clone())
        .recent_for_buyer(actor)
        .await
    {
        Ok(rows) => rows,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let preflight = PgDirectRefundPreflight::new(pool);
    let mut body = String::from(
        "<p>Заявка не гарантирует возврат. Разрешены только неоткрытые покупки
         с утверждённой при покупке политикой. Решение повторно проверяет права
         и срок, а возврат здесь относится только к внутреннему кошельку.</p>",
    );
    if rows.is_empty() {
        body.push_str("<p>Покупок пока нет.</p>");
    }
    for row in &rows {
        let allowed = row.request_id.is_none()
            && preflight.refund_gate(actor, row.purchase_id).await
                == DirectRefundGate::FirstGrantNotSeenInTrackedPurchase;
        body.push_str(&card(row, &csrf, allowed));
    }
    noindex(private_response(shell(&body), &cookies))
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<RefundForm>,
) -> Response {
    if !enabled(&state) {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Some(purchase), Ok(key)) = (
        positive_id(&form.purchase_id),
        IdempotencyKey::parse(&form.command_key),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    match PgDirectRefundRequests::new(state.accounts.pool().clone())
        .request_once(actor, purchase, &key, &form.reason)
        .await
    {
        Ok(receipt) => {
            let note = if receipt.created {
                "Заявка создана"
            } else {
                "Заявка уже существовала"
            };
            noindex(private_response(
                (
                    if receipt.created {
                        StatusCode::CREATED
                    } else {
                        StatusCode::OK
                    },
                    shell(&format!(
                        "<p role=\"status\">{note}: №{} по покупке №{}.
                         Это не одобрение возврата; денег или прав чтения
                         создание заявки не меняет.</p>
                         <p><a class=\"button\" href=\"/me/refunds\">История заявок</a></p>",
                        receipt.request_id, receipt.purchase_id
                    )),
                ),
                &[],
            ))
        }
        Err(e) => {
            let (status, message) = request_error(e);
            error(status, message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s036_buyer_refund_form_does_not_expose_unsafe_fields_or_approval() {
        assert_eq!(positive_id("0003"), Some(3));
        assert_eq!(positive_id("-1"), None);
        assert_eq!(positive_id("3x"), None);
        assert_eq!(positive_id("0"), None);
        assert_eq!(
            state_label("approved"),
            "Одобрена: внутренний возврат завершён"
        );
        let purchase = BuyerRefundPurchase {
            purchase_id: 12,
            book_title: "Книга <script>x</script>".into(),
            chapter_title: "Часть <img>".into(),
            amount_minor: 300,
            request_id: None,
            request_state: None,
        };
        let html = card(&purchase, "csrf", true);
        assert!(html.contains("action=\"/me/refunds\""));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("name=\"user_id\""));
        assert!(!html.contains("approve"));
        assert!(!card(&purchase, "csrf", false).contains("<form"));
    }
}
