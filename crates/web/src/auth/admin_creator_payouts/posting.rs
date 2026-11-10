//! S039: default-off administrator HTTP posting of INTERNAL payout obligation.
//! The password is verified and a one-use session proof consumed server-side.
//! Never an external provider transfer or bank settlement.
use super::super::{deny, session_user, AuthState};
use super::shell;
use axum::{
    extract::{rejection::FormRejection, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use tor_accounts::{
    repositories::step_up::StepUpIssueError,
    services::security::{session_token_digest, verify_password_for_step_up},
};
use tor_commerce::repositories::creator_payout_posting::{
    payout_step_up_payload_digest, PayoutPostError, PgCreatorPayoutPosting,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PostForm {
    csrf_token: String,
    expected_amount_minor: String,
    reason: String,
    current_password: String,
}

pub(super) fn enabled() -> bool {
    std::env::var("TOR_S039_PAYOUT_POST_HTTP_ENABLED").as_deref() == Ok("1")
}

pub(super) fn form(request_id: i64, amount_minor: i64, csrf: &str) -> String {
    format!(
        r#"<form method="post" action="/admin/finance/payouts/{request_id}/post"
          autocomplete="off">
        <h3>Внутренняя проводка по заявке №{request_id}</h3>
        <input type="hidden" name="expected_amount_minor" value="{amount_minor}">
        <input type="hidden" name="csrf_token" value="{}">
        <label>Основание финансового решения
          <textarea name="reason" maxlength="500" required></textarea></label>
        <label>Текущий пароль администратора
          <input type="password" name="current_password"
                 autocomplete="current-password" required></label>
        <p>Выполнит списание с единого внутреннего кошелька и создаст
        неизменяемую проводку. Денежные средства на банковский счёт
        через эту операцию НЕ перечисляются.</p>
        <button type="submit">Создать внутреннюю проводку</button>
        </form>"#,
        crate::pages::escape_html(csrf)
    )
}

fn amount(raw: &str) -> Option<i64> {
    if raw.is_empty() || raw.len() > 19 || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    raw.parse()
        .ok()
        .filter(|v| (1..=1_000_000_000_000).contains(v))
}

fn fail(status: StatusCode, message: &'static str) -> Response {
    let mut response = shell(
        &format!(
            r#"<p role="alert">{message}</p>
               <p><a href="/admin/finance/payouts">К заявкам</a></p>"#
        ),
        &[],
    );
    *response.status_mut() = status;
    response
}

fn issue_error(err: StepUpIssueError) -> StatusCode {
    match err {
        StepUpIssueError::InvalidIntent => StatusCode::UNPROCESSABLE_ENTITY,
        StepUpIssueError::Forbidden => StatusCode::FORBIDDEN,
        StepUpIssueError::StepUpRequired => StatusCode::UNAUTHORIZED,
        StepUpIssueError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn posting_error(err: PayoutPostError) -> StatusCode {
    match err {
        PayoutPostError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        PayoutPostError::Forbidden => StatusCode::FORBIDDEN,
        PayoutPostError::NotFound => StatusCode::NOT_FOUND,
        PayoutPostError::Conflict | PayoutPostError::Insufficient => StatusCode::CONFLICT,
        PayoutPostError::StepUpRequired => StatusCode::UNAUTHORIZED,
        PayoutPostError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(request_id): Path<i64>,
    headers: HeaderMap,
    submitted: Result<Form<PostForm>, FormRejection>,
) -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(user)) => user,
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
    let Some(amount_minor) = amount(&form.expected_amount_minor) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let reason = form.reason.trim();
    if request_id <= 0
        || reason.is_empty()
        || reason.chars().count() > 500
        || form.current_password.is_empty()
        || form.current_password.len() > 512
    {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let Some(session_token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let session_digest = session_token_digest(&session_token);
    let attempt_key = session_token_digest(&format!("finance:payout:stepup:{actor}"));
    match state
        .accounts
        .permit_auth_attempt(&attempt_key, 6, 600)
        .await
    {
        Ok(true) => {}
        Ok(false) => return fail(StatusCode::TOO_MANY_REQUESTS, "Превышен лимит попыток."),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let password_hash = match state.accounts.password_hash_for_user(actor).await {
        Ok(Some(hash)) => hash,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let password = form.current_password;
    let verified = match state
        .password_jobs
        .run(move || verify_password_for_step_up(&password, &password_hash))
        .await
    {
        Ok(Some(proof)) => proof,
        Ok(None) => return fail(StatusCode::UNAUTHORIZED, "Пароль не подтверждён."),
        Err(code) => return deny(code),
    };
    let digest = payout_step_up_payload_digest(actor, request_id, amount_minor, reason);
    let proof_token = match state
        .accounts
        .issue_administrator_payout_step_up(
            actor,
            &session_digest,
            &verified,
            request_id,
            amount_minor,
            &digest,
        )
        .await
    {
        Ok(token) => token,
        Err(err) => return deny(issue_error(err)),
    };
    let result = PgCreatorPayoutPosting::new(state.accounts.pool().clone())
        .approve_once(
            actor,
            &session_digest,
            &proof_token,
            request_id,
            amount_minor,
            reason,
        )
        .await;
    match result {
        Ok(receipt) => {
            let _ = state.accounts.clear_auth_attempt(&attempt_key).await;
            shell(
                &format!(
                    r#"<p role="status">По заявке №{} создана внутренняя проводка
                    №{} на {} коп. {}.
                    Банковский перевод НЕ выполнен и НЕ подтверждён.</p>
                    <p><a href="/admin/finance/payouts">К заявкам</a></p>"#,
                    receipt.request_id,
                    receipt.wallet_entry_id,
                    receipt.amount_minor,
                    if receipt.replayed { "Повтор без нового списания" } else { "Одно списание" },
                ),
                &[],
            )
        }
        Err(err) => fail(
            posting_error(err),
            "Внутренняя проводка не создана: статус, резерв, сумма, права или подтверждение изменились.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s039_post_form_does_not_claim_bank_settlement_or_trust_amount() {
        let html = form(9, 600, "csrf");
        assert!(html.contains("/admin/finance/payouts/9/post"));
        assert!(html.contains("name=\"expected_amount_minor\" value=\"600\""));
        assert!(html.contains("name=\"current_password\""));
        assert!(html.contains("name=\"csrf_token\""));
        assert!(!html.contains("name=\"recipient_user_id\""));
        assert!(!html.contains("name=\"admin_user_id\""));
        assert!(html.contains("НЕ перечисляются"));
        assert_eq!(amount("600"), Some(600));
        assert_eq!(amount("0"), None);
        assert_eq!(amount("-1"), None);
        assert_eq!(amount("1e3"), None);
    }
}
