//! S036: explicit opt-in rejection of a buyer refund request.
//! A fresh password and session-bound one-use rejection proof are required.
//! Rejection mutates only the request state, never wallets, purchases or rights.
use super::super::{deny, session_user, AuthState};
use super::{page_shell, AdminRefundRequest};
use axum::{
    extract::{rejection::FormRejection, Form, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use tor_accounts::services::security::{session_token_digest, verify_password_for_step_up};
use tor_commerce::repositories::refund_decisions::{
    rejection_step_up_payload_digest, DirectRefundRejectError, PgDirectRefundDecisions,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RejectionForm {
    request_id: String,
    expected_revision: String,
    reason: String,
    current_password: String,
    csrf_token: String,
}

/// A denial is allowed even when no approved commercial refund policy exists;
/// it can never issue a credit or simulate payment settlement.
pub(super) fn enabled(state: &AuthState) -> bool {
    state.paid_checkout_enabled
        && state.refund_browser_enabled
        && std::env::var("TOR_REFUND_REJECTION_HTTP_ENABLED").as_deref() == Ok("1")
}

pub(super) fn form(row: &AdminRefundRequest, csrf: &str) -> String {
    if row.state != "pending" {
        return String::new();
    }
    format!(
        r#"<form method="post" action="/admin/finance/refunds/reject" autocomplete="off">
           <input type="hidden" name="request_id" value="{}">
           <input type="hidden" name="expected_revision" value="{}">
           <input type="hidden" name="csrf_token" value="{}">
           <label>Причина отказа в возврате
             <textarea name="reason" maxlength="1000" required></textarea>
           </label>
           <label>Подтверждение текущим паролем администратора
             <input type="password" name="current_password"
                    autocomplete="current-password" required>
           </label>
           <p>Отказ в заявке не меняет баланс, покупку или право чтения.
           Сервер проверит роль, сессию, номер версии и одноразовое подтверждение.</p>
           <button type="submit">Отклонить заявку</button></form>"#,
        row.id,
        row.revision,
        crate::pages::escape_html(csrf)
    )
}

fn positive(raw: &str) -> Option<i64> {
    if raw.is_empty() || raw.len() > 19 || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    raw.parse::<i64>().ok().filter(|x| *x > 0)
}

fn fail(status: StatusCode, message: &'static str) -> Response {
    let mut response = page_shell(&format!(r#"<p role="alert">{message}</p>"#), &[]);
    *response.status_mut() = status;
    response
}

fn rejection_error(err: DirectRefundRejectError) -> (StatusCode, &'static str) {
    match err {
        DirectRefundRejectError::InvalidRequest => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Некорректный запрос. Обновите страницу и попробуйте снова.",
        ),
        DirectRefundRejectError::Forbidden => (
            StatusCode::FORBIDDEN,
            "Недостаточно полномочий для решения по заявке.",
        ),
        DirectRefundRejectError::StepUpRequired | DirectRefundRejectError::InvalidStepUp => (
            StatusCode::UNAUTHORIZED,
            "Повторное подтверждение личности не прошло.",
        ),
        DirectRefundRejectError::NotFound => (StatusCode::NOT_FOUND, "Заявка не найдена."),
        DirectRefundRejectError::RevisionConflict | DirectRefundRejectError::Conflict => (
            StatusCode::CONFLICT,
            "Решение не принято: состояние или версия заявки изменились.",
        ),
        DirectRefundRejectError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Проверка заявки временно недоступна.",
        ),
    }
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    submitted: Result<Form<RejectionForm>, FormRejection>,
) -> Response {
    // Do not parse malformed form fields before the independent opt-in gate.
    if !enabled(&state) {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let Form(form) = match submitted {
        Ok(form) => form,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Some(request), Some(revision)) = (
        positive(&form.request_id),
        positive(&form.expected_revision),
    ) else {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Неверный идентификатор заявки.",
        );
    };
    let reason = form.reason.trim();
    if reason.is_empty()
        || reason.chars().count() > 1000
        || form.current_password.is_empty()
        || form.current_password.len() > 512
    {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Заполните причину и пароль.",
        );
    }
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let session_digest = session_token_digest(&session);
    // Shared per-administrator attempt budget; no unbounded Argon2 worker queue.
    let attempt_key = session_token_digest(&format!("finance:refund:reject-stepup:{actor}"));
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
        Err(status) => return deny(status),
    };
    let digest = rejection_step_up_payload_digest(actor, request, revision, reason);
    let proof_token = match state
        .accounts
        .issue_administrator_refund_rejection_step_up(
            actor,
            &session_digest,
            &verified,
            request,
            revision,
            &digest,
        )
        .await
    {
        Ok(token) => token,
        Err(tor_accounts::repositories::step_up::StepUpIssueError::Forbidden) => {
            return deny(StatusCode::FORBIDDEN);
        }
        Err(tor_accounts::repositories::step_up::StepUpIssueError::InvalidIntent) => {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        }
        Err(tor_accounts::repositories::step_up::StepUpIssueError::StepUpRequired) => {
            return deny(StatusCode::UNAUTHORIZED);
        }
        Err(tor_accounts::repositories::step_up::StepUpIssueError::Unavailable) => {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    // No token is written to HTML, storage, logs or a client cookie.
    // Approval consumes the one-use session proof in the same PostgreSQL
    // transaction that credits the wallet and revokes the reading entitlement.
    let result = PgDirectRefundDecisions::new(state.accounts.pool().clone())
        .reject_once(
            actor,
            &session_digest,
            &proof_token,
            request,
            revision,
            reason,
        )
        .await;
    match result {
        Ok(receipt) => {
            let _ = state.accounts.clear_auth_attempt(&attempt_key).await;
            page_shell(
                &format!(
                    r#"<p role="status">Заявка №{} отклонена.
                       {}. Финансовые операции не выполнялись.</p>
                       <p><a href="/admin/finance/refunds">К заявкам</a></p>"#,
                    receipt.request_id,
                    if receipt.replayed { "Повтор решения" } else { "Решение сохранено" },
                ),
                &[],
            )
        }
        Err(err) => {
            let (status, message) = rejection_error(err);
            fail(status, message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s036_rejection_only_pending_and_requires_password() {
        let row = AdminRefundRequest {
            id: 7,
            purchase_id: 9,
            buyer_user_id: 12,
            buyer_name: "Name".into(),
            book_title: "Book".into(),
            chapter_title: "Chapter".into(),
            amount_minor: 300,
            reason: "Buyer reason".into(),
            state: "pending".into(),
            revision: 3,
            policy_version: None,
            deadline_utc: None,
            created_utc: "2026-11-03".into(),
            first_exposed: true,
            revoked: false,
        };
        let markup = form(&row, "csrf");
        assert!(markup.contains("action=\"/admin/finance/refunds/reject\""));
        assert!(markup.contains("name=\"current_password\""));
        assert!(!markup.contains("name=\"buyer_user_id\""));
        assert!(!markup.contains("name=\"amount_minor\""));
        let mut terminal = row;
        terminal.state = "approved".into();
        assert!(form(&terminal, "csrf").is_empty());
        assert!(positive("0").is_none());
        assert_eq!(positive("0008"), Some(8));
    }
}
