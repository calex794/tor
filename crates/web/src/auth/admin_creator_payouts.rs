//! S039/D-09: closed administrator payout queue and rejection-only action.
//! Internal wallet posting is separately opt-in; no external bank settlement.
mod posting;
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{rejection::FormRejection, DefaultBodyLimit, Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_commerce::repositories::creator_payout_admin::{
    PayoutAdminError, PayoutAdminRow, PgCreatorPayoutAdmin,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueQuery {
    before: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectionForm {
    csrf_token: String,
    reason: String,
}

// Independent of the author request gate: operators may need to release
// existing holds even while creating new requests remains disabled.
fn rejection_enabled() -> bool {
    std::env::var("TOR_S039_PAYOUT_REJECT_HTTP_ENABLED").as_deref() == Ok("1")
}

fn quote_enabled() -> bool {
    std::env::var("TOR_S039_PAYOUT_QUOTE_HTTP_ENABLED").as_deref() == Ok("1")
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/admin/finance/payouts", get(queue))
        .route("/admin/finance/payouts/{id}/quote", get(quote))
        .route(
            "/admin/finance/payouts/{id}/reject",
            post(reject).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/admin/finance/payouts/{id}/post",
            post(posting::submit).layer(DefaultBodyLimit::max(4096)),
        )
}

fn status(error: PayoutAdminError) -> StatusCode {
    match error {
        PayoutAdminError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        PayoutAdminError::NotFound => StatusCode::NOT_FOUND,
        PayoutAdminError::Conflict => StatusCode::CONFLICT,
        PayoutAdminError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn shell(content: &str, cookies: &[String]) -> Response {
    let mut response = private_response(
        pages::layout(
            "Заявки на выплату — администратор",
            "Приватный аудит и отклонение ожидающих выплат.",
            &format!(
                r#"<section class="section"><div class="container">
                <h1>Заявки на выплату</h1>
                <p>Внутренние резервы и проводки. Отклонение ожидающей заявки
                освобождает резерв и не меняет баланс кошелька. Банковские
                перечисления через эту страницу не выполняются.</p>
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

fn render_request(row: &PayoutAdminRow, csrf: &str, allow_reject: bool, allow_post: bool) -> String {
    let label = if row.state == "pending" {
        "Ожидает решения (не перечислена)"
    } else if row.state == "rejected" {
        "Отклонена; резерв освобождён"
    } else if row.state == "posted" {
        "Внутренняя проводка создана; банковский перевод не подтверждён"
    } else {
        "Неизвестное состояние"
    };
    let mut html = format!(
        r#"<article class="me-now-card">
        <h2>Заявка №{} — {}</h2>
        <p>Получатель №{} · {} коп. · UTC {}</p>"#,
        row.id,
        label,
        row.user_id,
        row.amount_minor,
        pages::escape_html(&row.created_utc),
    );
    if allow_reject && row.state == "pending" {
        write!(
            html,
            r#"<form method="post" action="/admin/finance/payouts/{}/reject"
                autocomplete="off">
              <input type="hidden" name="csrf_token" value="{}">
              <label>Мотивировка отклонения
                <textarea name="reason" maxlength="500" required></textarea>
              </label>
              <button type="submit">Отклонить и освободить резерв</button>
              </form>"#,
            row.id,
            pages::escape_html(csrf),
        )
        .expect("bounded rejection form");
    }
    if allow_post && row.state == "pending" {
        write!(
            html,
            r#"<form method="post" action="/admin/finance/payouts/{}/post"
                autocomplete="off">
              <input type="hidden" name="csrf_token" value="{}">
              <label>Основание внутреннего списания
                <textarea name="reason" maxlength="500" required></textarea>
              </label>
              <label>Текущий пароль администратора
                <input type="password" name="current_password"
                       autocomplete="current-password" required>
              </label>
              <p>Действие создаёт внутреннюю проводку и списывает средства
              из единого кошелька. Банковский перевод не выполняется.</p>
              <button type="submit">Создать внутреннюю проводку</button>
              </form>"#,
            row.id,
            pages::escape_html(csrf),
        )
        .expect("bounded internal posting form");
    }
    html.push_str("</article>");
    html
}

async fn queue(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<QueueQuery>,
) -> Response {
    match session_user(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(code) => return deny(code),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    let rows = match PgCreatorPayoutAdmin::new(state.accounts.pool().clone())
        .list(&digest, query.before)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(status(error)),
    };
    let enabled = rejection_enabled();
    let can_post = posting::enabled();
    let (csrf, cookies) = if enabled || can_post {
        state.cookies.csrf_for_page(&headers)
    } else {
        (String::new(), Vec::new())
    };
    let mut body = String::new();
    if !enabled {
        body.push_str("<p>Административное отклонение пока отключено.</p>");
    }
    if rows.is_empty() {
        body.push_str("<p>Заявок пока нет.</p>");
    }
    for row in &rows {
        body.push_str(&render_request(row, &csrf, enabled, can_post));
        if quote_enabled() && row.state == "pending" {
            write!(
                body,
                r#"<p><a href="/admin/finance/payouts/{}/quote">
                Проверить резерв (без перевода)</a></p>"#,
                row.id
            )
            .expect("bounded quote link");
        }
    }
    if rows.len() == 30 {
        if let Some(last) = rows.last() {
            write!(
                body,
                r#"<p><a href="/admin/finance/payouts?before={}">
                Предыдущие заявки</a></p>"#,
                last.id
            )
            .expect("bounded payout cursor");
        }
    }
    shell(&body, &cookies)
}

async fn quote(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !quote_enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    match session_user(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(code) => return deny(code),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    let q = match PgCreatorPayoutAdmin::new(state.accounts.pool().clone())
        .approval_readiness(&digest, id)
        .await
    {
        Ok(q) => q,
        Err(error) => return deny(status(error)),
    };
    let verdict = if q.sufficient {
        "Доступных средств достаточно по текущему снимку"
    } else {
        "Текущих доступных средств недостаточно"
    };
    let body = format!(
        r#"<article class="me-now-card">
        <h2>Проверка заявки №{}</h2>
        <p>Получатель №{} · сумма: {} коп.</p>
        <p>Резерв других заявок: {} коп.</p>
        <p>Доступно без резерва этой заявки: {} коп.</p>
        <p><strong>{verdict}</strong></p>
        <p>Предварительный расчёт, не подтверждение, списание или банковский
        перевод. При финансовом решении сервер обязан повторить проверку
        и записать все изменения в одной транзакции.</p>
        </article>
        <p><a href="/admin/finance/payouts">К очереди заявок</a></p>"#,
        q.request_id,
        q.user_id,
        q.amount_minor,
        q.other_reserved_minor,
        q.available_excluding_own_minor,
    );
    shell(&body, &[])
}

async fn reject(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    submitted: Result<Form<RejectionForm>, FormRejection>,
) -> Response {
    if !rejection_enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    match session_user(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(code) => return deny(code),
    };
    let Form(form) = match submitted {
        Ok(form) => form,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match PgCreatorPayoutAdmin::new(state.accounts.pool().clone())
        .reject_once(&digest, id, &form.reason)
        .await
    {
        Ok(_) => private_response(Redirect::to("/admin/finance/payouts"), &[]).into_response(),
        Err(error) => deny(status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s039_reject_button_only_on_pending_and_no_banking_claim() {
        let mut row = PayoutAdminRow {
            id: 9,
            user_id: 11,
            amount_minor: 300,
            state: "pending".into(),
            created_utc: "2026-11-10 00:00".into(),
        };
        let html = render_request(&row, "csrf", true, false);
        assert!(html.contains("action=\"/admin/finance/payouts/9/reject\""));
        assert!(html.contains("name=\"csrf_token\""));
        assert!(!html.contains("name=\"amount_minor\""));
        assert!(!html.contains("Подтвердить выплату"));
        row.state = "rejected".into();
        assert!(!render_request(&row, "csrf", true, false).contains("<form"));
        row.state = "posted".into();
        let posted = render_request(&row, "csrf", true, false);
        assert!(posted.contains("Внутренняя проводка создана"));
        assert!(posted.contains("банковский перевод не подтверждён"));
        assert!(!posted.contains("<form"));
        row.state = "pending".into();
        assert!(!render_request(&row, "csrf", false, false).contains("<form"));
        row.created_utc = "<script>bad</script>".into();
        assert!(!render_request(&row, "csrf", true, false).contains("<script>"));
        row.created_utc = "2026-11-10 00:00".into();
        let post_form = render_request(&row, "csrf", false, true);
        assert!(post_form.contains("action=\"/admin/finance/payouts/9/post\""));
        assert!(post_form.contains("name=\"current_password\""));
        assert!(post_form.contains("name=\"csrf_token\""));
        assert!(!post_form.contains("name=\"amount_minor\""));
        assert!(post_form.contains("Банковский перевод не выполняется"));
        row.state = "posted".into();
        assert!(!render_request(&row, "csrf", false, true).contains("<form"));
    }
}
