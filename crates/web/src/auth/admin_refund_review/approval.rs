//! S036: explicitly opt-in administrator approval of an internal refund.
//! A password-confirmed, session-bound one-use proof is issued immediately
//! before the existing D-06 PostgreSQL transaction. No external payout API.
use super::super::{deny, session_user, AuthState};
use super::{page_shell, AdminRefundRequest};
use axum::{
    extract::{rejection::FormRejection, Form, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use tor_accounts::services::security::{session_token_digest, verify_password_for_step_up};
use tor_commerce::repositories::refund_approvals::{
    approval_step_up_payload_digest, DirectRefundApproveError, PgDirectRefundApprovals,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ApprovalForm {
    request_id: String,
    expected_revision: String,
    reason: String,
    current_password: String,
    csrf_token: String,
}

/// The approved policy catalog and checkout snapshots are independent mandatory
/// conditions enforced again by PgDirectRefundApprovals::approve_once.
pub(super) fn enabled(state: &AuthState) -> bool {
    state.paid_checkout_enabled
        && state.refund_browser_enabled
        && std::env::var("TOR_REFUND_APPROVAL_HTTP_ENABLED").as_deref() == Ok("1")
}

pub(super) fn form(row: &AdminRefundRequest, csrf: &str) -> String {
    if row.state != "pending"
        || row.policy_version.is_none()
        || row.deadline_utc.is_none()
        || row.first_exposed
        || row.revoked
    {
        return String::new();
    }
    format!(
        r#"<form method="post" action="/admin/finance/refunds/approve" autocomplete="off">
           <input type="hidden" name="request_id" value="{}">
           <input type="hidden" name="expected_revision" value="{}">
           <input type="hidden" name="csrf_token" value="{}">
           <label>Причина административного решения
             <textarea name="reason" maxlength="1000" required></textarea>
           </label>
           <label>Подтверждение текущим паролем администратора
             <input type="password" name="current_password"
                    autocomplete="current-password" required>
           </label>
           <p>Разрешён только внутренний возврат в кошелёк по утверждённой
           политике. Срок, открытие главы, версию решения и бухгалтерский
           баланс сервер повторно проверит в одной транзакции.</p>
           <button type="submit">Подтвердить внутренний возврат</button></form>"#,
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

fn approval_error(err: DirectRefundApproveError) -> (StatusCode, &'static str) {
    match err {
        DirectRefundApproveError::InvalidRequest => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Некорректный запрос. Обновите страницу и попробуйте снова.",
        ),
        DirectRefundApproveError::Forbidden => (
            StatusCode::FORBIDDEN,
            "Недостаточно полномочий для финансового решения.",
        ),
        DirectRefundApproveError::StepUpRequired | DirectRefundApproveError::InvalidStepUp => (
            StatusCode::UNAUTHORIZED,
            "Повторное подтверждение личности не прошло.",
        ),
        DirectRefundApproveError::NotFound => (StatusCode::NOT_FOUND, "Заявка не найдена."),
        DirectRefundApproveError::RevisionConflict
        | DirectRefundApproveError::Conflict
        | DirectRefundApproveError::PolicyUnavailable
        | DirectRefundApproveError::WindowExpired
        | DirectRefundApproveError::AlreadyOpened
        | DirectRefundApproveError::Revoked
        | DirectRefundApproveError::UnverifiedPurchase
        | DirectRefundApproveError::Overflow => (
            StatusCode::CONFLICT,
            "Решение отклонено: статус, версия, условия возврата или финансовые данные изменились.",
        ),
        DirectRefundApproveError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Проверка возврата временно недоступна.",
        ),
    }
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    submitted: Result<Form<ApprovalForm>, FormRejection>,
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
    let attempt_key = session_token_digest(&format!("finance:refund:stepup:{actor}"));
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
    let digest = approval_step_up_payload_digest(actor, request, revision, reason);
    let proof_token = match state
        .accounts
        .issue_administrator_refund_step_up(
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
    let result = PgDirectRefundApprovals::new(state.accounts.pool().clone())
        .approve_once(
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
                    r#"<p role="status">Решение по заявке №{} принято.
                       Внутренний возврат: {} коп.
                       {}.</p><p><a href="/admin/finance/refunds">К заявкам</a></p>"#,
                    receipt.request_id,
                    receipt.credited_minor,
                    if receipt.replayed {
                        "Повторное подтверждение"
                    } else {
                        "Один перевод в кошелёк"
                    },
                ),
                &[],
            )
        }
        Err(err) => {
            let (status, message) = approval_error(err);
            fail(status, message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s036_approval_form_is_eligible_only_and_omits_sensitive_authority_fields() {
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
            policy_version: Some("policy".into()),
            deadline_utc: Some("2026-11-03".into()),
            created_utc: "2026-11-03".into(),
            first_exposed: false,
            revoked: false,
        };
        let markup = form(&row, "csrf");
        assert!(markup.contains("name=\"request_id\" value=\"7\""));
        assert!(markup.contains("name=\"expected_revision\" value=\"3\""));
        assert!(markup.contains("name=\"current_password\""));
        assert!(!markup.contains("name=\"buyer_user_id\""));
        assert!(!markup.contains("name=\"amount_minor\""));
        assert!(!markup.contains("name=\"decision_actor\""));
        let mut unavailable = row;
        unavailable.first_exposed = true;
        assert!(form(&unavailable, "csrf").is_empty());
        unavailable.first_exposed = false;
        unavailable.policy_version = None;
        assert!(form(&unavailable, "csrf").is_empty());
        assert!(positive("0").is_none());
        assert!(positive("-1").is_none());
        assert_eq!(positive("0008"), Some(8));
    }
}
