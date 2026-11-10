//! S039/MONEY-04: password-confirmed administrator HTTP posting.
//! A successful response means a wallet debit, NEVER an external bank transfer.
use super::{deny, session_user, shell, status, AuthState};
use axum::{
    extract::{rejection::FormRejection, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use tor_accounts::services::security::{session_token_digest, verify_password_for_step_up};
use tor_commerce::repositories::{
    creator_payout_admin::PgCreatorPayoutAdmin,
    creator_payout_posting::{
        payout_step_up_payload_digest, PayoutPostError, PgCreatorPayoutPosting,
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PostingForm {
    csrf_token: String,
    reason: String,
    current_password: String,
}

// A distinct operator-only gate. Does NOT enable bank/provider transfers.
pub(super) fn enabled() -> bool {
    std::env::var("TOR_S039_PAYOUT_POST_HTTP_ENABLED").as_deref() == Ok("1")
}

fn fail(code: StatusCode, message: &'static str) -> Response {
    let mut response = shell(&format!(r#"<p role="alert">{message}</p>"#), &[]);
    *response.status_mut() = code;
    response
}

fn posting_error(error: PayoutPostError) -> (StatusCode, &'static str) {
    match error {
        PayoutPostError::Invalid => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Неверные параметры внутренней проводки.",
        ),
        PayoutPostError::Forbidden => (
            StatusCode::FORBIDDEN,
            "Недостаточно полномочий для этого финансового решения.",
        ),
        PayoutPostError::NotFound => (StatusCode::NOT_FOUND, "Заявка не найдена."),
        PayoutPostError::Conflict | PayoutPostError::Insufficient => (
            StatusCode::CONFLICT,
            "Заявка изменена или средств недостаточно. Обновите очередь.",
        ),
        PayoutPostError::StepUpRequired => (
            StatusCode::UNAUTHORIZED,
            "Повторное подтверждение паролем не прошло.",
        ),
        PayoutPostError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Финансовая операция временно недоступна.",
        ),
    }
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    submitted: Result<Form<PostingForm>, FormRejection>,
) -> Response {
    // Do not even parse or validate the request body unless explicitly enabled.
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
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
    let reason = form.reason.trim();
    if id <= 0
        || reason.is_empty()
        || reason.chars().count() > 500
        || form.current_password.is_empty()
        || form.current_password.len() > 512
    {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);

    // The amount is read from an audited exact PostgreSQL request snapshot,
    // including an already-posted request for safe retries. Never trust the form.
    let intent = match PgCreatorPayoutAdmin::new(state.accounts.pool().clone())
        .posting_intent(&digest, id)
        .await
    {
        Ok(intent) => intent,
        Err(error) => return deny(status(error)),
    };
    if intent.user_id == actor {
        return deny(StatusCode::FORBIDDEN);
    }

    // Same bounded Argon2 worker and attempt-budget pattern as admin refunds.
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

    // Bind the proof to the immutable request amount and normalized reason.
    // Never expose the one-use secret in a response, form, cookie or log.
    let payload = payout_step_up_payload_digest(actor, id, intent.amount_minor, reason);
    let proof = match state
        .accounts
        .issue_administrator_payout_step_up(
            actor,
            &digest,
            &verified,
            id,
            intent.amount_minor,
            &payload,
        )
        .await
    {
        Ok(token) => token,
        Err(tor_accounts::repositories::step_up::StepUpIssueError::InvalidIntent) => {
            return deny(StatusCode::UNPROCESSABLE_ENTITY);
        }
        Err(tor_accounts::repositories::step_up::StepUpIssueError::Forbidden) => {
            return deny(StatusCode::FORBIDDEN);
        }
        Err(tor_accounts::repositories::step_up::StepUpIssueError::StepUpRequired) => {
            return deny(StatusCode::UNAUTHORIZED);
        }
        Err(tor_accounts::repositories::step_up::StepUpIssueError::Unavailable) => {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    // Role, live session, amount, reserves, proof and debit are re-checked in
    // the single S039 transaction. Its replay returns the same wallet entry.
    let result = PgCreatorPayoutPosting::new(state.accounts.pool().clone())
        .approve_once(actor, &digest, &proof, id, intent.amount_minor, reason)
        .await;
    match result {
        Ok(receipt) => {
            let _ = state.accounts.clear_auth_attempt(&attempt_key).await;
            shell(
                &format!(
                    r#"<p role="status">По заявке №{} создана внутренняя проводка
                    на {} коп. Запись кошелька №{}.
                    {}</p><p><strong>Банковский перевод не выполнялся и не
                    подтверждён.</strong></p>
                    <p><a href="/admin/finance/payouts">К очереди заявок</a></p>"#,
                    receipt.request_id,
                    receipt.amount_minor,
                    receipt.wallet_entry_id,
                    if receipt.replayed {
                        "Повторное подтверждение: списание не повторялось."
                    } else {
                        "Средства списаны с внутреннего кошелька один раз."
                    },
                ),
                &[],
            )
        }
        Err(error) => {
            let (code, message) = posting_error(error);
            fail(code, message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::posting_error;
    use axum::http::StatusCode;
    use tor_commerce::repositories::creator_payout_posting::PayoutPostError;

    #[test]
    fn s039_http_post_errors_never_claim_external_settlement() {
        let (code, message) = posting_error(PayoutPostError::Insufficient);
        assert_eq!(code, StatusCode::CONFLICT);
        assert!(!message.contains("банковский перевод выполнен"));
    }
}
