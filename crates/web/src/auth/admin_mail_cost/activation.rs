//! Explicit two-step activation. Review never enables mail.
use super::{operator, pages, profile, valid_csrf, AuthState};
use crate::{
    auth_cookies::{deny, private_response},
    flow_runtime::mail::MailControl,
};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    Extension,
};
use serde::Deserialize;
use tor_accounts::{
    repositories::step_up::StepUpIssueError,
    services::security::{session_token_digest, verify_password_for_step_up},
};
use tor_mail::{
    domain::{launch::LaunchSnapshot, postbox_profile::SealedProfile},
    repositories::launch::PgMailLaunch,
    services::launch::{LaunchError, LaunchScope},
};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewForm {
    csrf_token: String,
    revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfirmForm {
    csrf_token: String,
    challenge: String,
    evidence_sha256: String,
    reviewed: String,
    current_password: String,
}

fn decode32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut bytes = [0; 32];
    hex::decode_to_slice(value, &mut bytes).ok()?;
    (bytes != [0; 32]).then_some(bytes)
}

fn launch_status(error: LaunchError) -> StatusCode {
    match error {
        LaunchError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::CONFLICT,
    }
}

pub(super) fn entry(csrf: &str, revision: i64) -> String {
    let csrf = pages::escape_html(csrf);
    format!(
        "<section class=\"section\"><div class=\"container\"><h2>Активация отправки</h2>\
         <form method=\"post\" action=\"/odmen/system-mail/activation/review\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
         <input type=\"hidden\" name=\"revision\" value=\"{revision}\">\
         <button type=\"submit\">Подготовить активацию</button></form></div></section>"
    )
}

fn review_body(
    snapshot: &LaunchSnapshot,
    candidate: &SealedProfile,
    challenge: &[u8; 32],
    csrf: &str,
) -> Option<String> {
    let cost = snapshot.review_cost();
    let usage = cost.usage?;
    let limits = cost.limits;
    let estimate = usage.estimate_kopecks(0)?;
    let csrf = pages::escape_html(csrf);
    let month = pages::escape_html(snapshot.month());
    let sender = pages::escape_html(candidate.identity().sender());
    let identity = pages::escape_html(candidate.identity().identity_id());
    let evidence = hex::encode(snapshot.evidence());
    let challenge = hex::encode(challenge);
    Some(format!(
        "<section class=\"section\"><div class=\"container\"><h1>Подтверждение активации</h1>\
         <p>Профиль {identity}, отправитель {sender}. Месяц {month} (МСК).</p>\
         <p>Ревизия политики: {}. Попыток: {}. Бесплатная квота: {}.\
         Базовые расходы: {}. Тариф за 1000: {}. Оценка: {}. Бюджет: {}.</p>\
         <p>Пороги: {}, {}, {}. SHA-256 сверки: <code>{evidence}</code>.</p>\
         <p>Независимо проверьте документ сверки и профиль. Подтверждение действует пять минут.\
         После подтверждения обработчик сможет отправлять письма восстановления пароля.</p>\
         <form method=\"post\" action=\"/odmen/system-mail/activation/confirm\" autocomplete=\"off\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
         <input type=\"hidden\" name=\"challenge\" value=\"{challenge}\">\
         <label>SHA-256 проверенного документа <input name=\"evidence_sha256\" minlength=\"64\" maxlength=\"64\" required></label>\
         <label><input type=\"checkbox\" name=\"reviewed\" value=\"yes\" required> Я проверил профиль, расходы и разрешаю отправку</label>\
         <label>Текущий пароль <input type=\"password\" name=\"current_password\" maxlength=\"512\" autocomplete=\"current-password\" required></label>\
         <button type=\"submit\">Проверить SMTP и включить отправку</button></form>\
         <p><a href=\"/odmen/system-mail\">Отмена</a></p></div></section>",
        snapshot.policy_revision(),
        usage.reserved_attempts,
        usage.free_allowance,
        super::view::money(usage.baseline_kopecks),
        super::view::money(usage.rate_per_thousand),
        super::view::money(estimate),
        super::view::money(limits.monthly_limit),
        super::view::money(limits.warning_1),
        super::view::money(limits.warning_2),
        super::view::money(limits.news_pause),
    ))
}

pub(super) async fn review(
    State(state): State<AuthState>,
    headers: HeaderMap,
    control: Option<Extension<MailControl>>,
    Form(form): Form<ReviewForm>,
) -> Response {
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if !valid_csrf(&state, &headers, &form.csrf_token) || !profile::secure_origin(&state) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(Extension(control)) = control else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    let launch = PgMailLaunch::new(state.accounts.pool().clone());
    let (snapshot, candidate) = match launch.admin_review_candidate(&digest).await {
        Ok(value) => value,
        Err(error) => return deny(profile::status(error)),
    };
    if snapshot.policy_revision() != form.revision {
        return deny(StatusCode::CONFLICT);
    }
    let challenge = match control
        .fence()
        .begin_review(&snapshot, LaunchScope::Dispatch, digest)
    {
        Ok(value) => value,
        Err(error) => return deny(launch_status(error)),
    };
    let Some(body) = review_body(&snapshot, &candidate, &challenge, &form.csrf_token) else {
        return deny(StatusCode::CONFLICT);
    };
    private_response(
        pages::layout("Активация почты", "Подтверждение параметров", &body),
        &[],
    )
}

pub(super) async fn confirm(
    State(state): State<AuthState>,
    headers: HeaderMap,
    control: Option<Extension<MailControl>>,
    Form(form): Form<ConfirmForm>,
) -> Response {
    let password = Zeroizing::new(form.current_password);
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if !valid_csrf(&state, &headers, &form.csrf_token) || !profile::secure_origin(&state) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(Extension(control)) = control else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    let (Some(challenge), Some(evidence)) =
        (decode32(&form.challenge), decode32(&form.evidence_sha256))
    else {
        return deny(StatusCode::BAD_REQUEST);
    };
    if form.reviewed != "yes" {
        return deny(StatusCode::BAD_REQUEST);
    }
    let launch = PgMailLaunch::new(state.accounts.pool().clone());
    let (snapshot, candidate) = match launch.admin_review_candidate(&digest).await {
        Ok(value) => value,
        Err(error) => return deny(profile::status(error)),
    };
    if let Err(error) = control.fence().precheck_review(
        &challenge,
        &digest,
        &snapshot,
        LaunchScope::Dispatch,
        &evidence,
    ) {
        return deny(launch_status(error));
    }
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(value)) if value.administrator => value.actor.user_id.get(),
        Ok(_) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if password.is_empty() || password.len() > 512 {
        return deny(StatusCode::UNAUTHORIZED);
    }
    let attempt_key = session_token_digest(&format!("auth:mail-activation-stepup:{actor}"));
    match state
        .accounts
        .permit_auth_attempt(&attempt_key, 6, 600)
        .await
    {
        Ok(true) => {}
        Ok(false) => return deny(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let hash = match state.accounts.password_hash_for_user(actor).await {
        Ok(Some(hash)) => Zeroizing::new(hash),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let verified = match state
        .password_jobs
        .run(move || verify_password_for_step_up(&password, &hash))
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    let probe_key = session_token_digest(&format!("mail:postbox-probe:{actor}"));
    match state.accounts.permit_auth_attempt(&probe_key, 3, 60).await {
        Ok(true) => {}
        Ok(false) => return deny(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let master = match profile::master_key() {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if let Err(error) = profile::probe_candidate(&master, &candidate).await {
        return deny(error);
    }
    let (fresh, fresh_candidate) = match launch.admin_review_candidate(&digest).await {
        Ok(value) => value,
        Err(error) => return deny(profile::status(error)),
    };
    if fresh != snapshot || fresh_candidate.intent_digest() != candidate.intent_digest() {
        return deny(StatusCode::CONFLICT);
    }
    let intent = match control.fence().confirm_review(
        &challenge,
        &digest,
        &fresh,
        &evidence,
        &candidate.intent_digest(),
    ) {
        Ok(value) => value,
        Err(error) => return deny(launch_status(error)),
    };
    let proof = match state
        .accounts
        .issue_administrator_mail_activation_step_up(
            actor,
            &digest,
            &verified,
            intent.policy_revision(),
            &intent.proof_digest(),
        )
        .await
    {
        Ok(value) => Zeroizing::new(value),
        Err(StepUpIssueError::Forbidden) => return deny(StatusCode::NOT_FOUND),
        Err(StepUpIssueError::StepUpRequired) => return deny(StatusCode::UNAUTHORIZED),
        Err(StepUpIssueError::InvalidIntent) => return deny(StatusCode::BAD_REQUEST),
        Err(StepUpIssueError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if let Err(error) = launch.admin_activate(&digest, &proof, &intent).await {
        return deny(profile::status(error));
    }
    // A DB commit alone cannot arm sending. Never automatically roll back
    // an ambiguous commit: another operator could own a newer revision.
    let active = match launch.snapshot().await {
        Ok(Some(value)) => value,
        _ => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if let Err(error) = control.fence().complete_activation(intent, &active) {
        return deny(launch_status(error));
    }
    let _ = state.accounts.clear_auth_attempt(&attempt_key).await;
    private_response(Redirect::to("/odmen/system-mail"), &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_input_is_bounded_and_entry_does_not_confirm() {
        assert_eq!(decode32(&"ab".repeat(32)), Some([0xab; 32]));
        assert!(decode32(&"00".repeat(32)).is_none());
        assert!(decode32(&"ab".repeat(33)).is_none());
        assert!(decode32(&"gg".repeat(32)).is_none());
        let html = entry("<fixture>", 17);
        assert!(html.contains("&lt;fixture&gt;"));
        assert!(html.contains("/activation/review"));
        assert!(!html.contains("/activation/confirm"));
    }
}
