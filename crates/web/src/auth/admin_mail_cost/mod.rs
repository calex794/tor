//! External system mail cost page; never touches the internal message system.
use super::AuthState;
use crate::flow_runtime::mail::MailControl;
use crate::{
    auth_cookies::{deny, private_response},
    pages,
};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Extension, Router,
};
use serde::Deserialize;
use tor_accounts::{
    repositories::step_up::StepUpIssueError,
    services::security::{session_token_digest, verify_password_for_step_up},
};
use tor_mail::domain::postbox_profile::{ProfileIdentity, ProfileSecrets};
use tor_mail::repositories::cost::{
    admin::{CostAdminError, CostAdminView, LimitChange, MonthReconciliation},
    PgMailCost,
};

use zeroize::Zeroizing;

mod activation;
mod profile;
mod view;
use profile::{ProbeForm, ProfileForm};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/odmen/system-mail", get(page).post(save))
        .route("/odmen/system-mail/stop", post(stop))
        .route("/odmen/system-mail/reconcile", post(reconcile))
        .route("/odmen/system-mail/profile", post(save_profile))
        .route("/odmen/system-mail/probe", post(probe_profile))
        .route(
            "/odmen/system-mail/activation/review",
            post(activation::review),
        )
        .route(
            "/odmen/system-mail/activation/confirm",
            post(activation::confirm),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsForm {
    csrf_token: String,
    revision: i64,
    warning_1: i64,
    warning_2: i64,
    news_pause: i64,
    monthly_limit: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconcileForm {
    csrf_token: String,
    revision: i64,
    month: String,
    free_allowance: i64,
    baseline_kopecks: i64,
    rate_per_thousand: i64,
    reserved_attempts: i64,
    evidence_sha256: String,
    reviewed: String,
}

impl ReconcileForm {
    fn request(&self) -> Option<MonthReconciliation> {
        let month = self.month.as_bytes();
        if self.reviewed != "yes"
            || self.revision <= 0
            || month.len() != 7
            || month[4] != b'-'
            || !month[..4].iter().all(u8::is_ascii_digit)
            || !month[5..].iter().all(u8::is_ascii_digit)
            || !(1..=12).contains(&self.month[5..].parse::<u8>().ok()?)
            || self.evidence_sha256.len() != 64
        {
            return None;
        }
        let mut evidence_sha256 = [0; 32];
        hex::decode_to_slice(&self.evidence_sha256, &mut evidence_sha256).ok()?;
        if evidence_sha256 == [0; 32] {
            return None;
        }
        let usage = tor_mail::domain::cost::MonthUsage {
            month_start: format!("{}-01", self.month),
            free_allowance: self.free_allowance,
            baseline_kopecks: self.baseline_kopecks,
            rate_per_thousand: self.rate_per_thousand,
            reserved_attempts: self.reserved_attempts,
        };
        usage.estimate_kopecks(1)?;
        Some(MonthReconciliation {
            usage,
            evidence_sha256,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopForm {
    csrf_token: String,
}

async fn operator(state: &AuthState, headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::NOT_FOUND)?;
    let digest = session_token_digest(&token);
    let actor = state
        .accounts
        .session_actor(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if !actor.administrator {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(digest)
}

fn status(error: CostAdminError) -> StatusCode {
    match error {
        CostAdminError::Invalid => StatusCode::BAD_REQUEST,
        CostAdminError::Forbidden => StatusCode::NOT_FOUND,
        CostAdminError::Conflict => StatusCode::CONFLICT,
        CostAdminError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn store(state: &AuthState) -> PgMailCost {
    PgMailCost::new(state.accounts.pool().clone())
}

async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    control: Option<Extension<MailControl>>,
) -> Response {
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    let current = match store(&state).admin_view(&digest).await {
        Ok(value) => value,
        Err(error) => return deny(status(error)),
    };
    let saved = match profile::store(&state).admin_view(&digest).await {
        Ok(value) => value,
        Err(error) => return deny(profile::status(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut body = view::body(&current, &csrf);
    let key_ready = profile::master_key().is_ok() && profile::secure_origin(&state);
    body.push_str(&profile::body(
        &saved,
        &csrf,
        key_ready,
        current.snapshot.limits.enabled,
    ));
    if control.is_some()
        && key_ready
        && !current.snapshot.limits.enabled
        && saved.identity.is_some()
    {
        body.push_str(&activation::entry(&csrf, current.revision));
    }
    let mut response = private_response(
        pages::layout("Системная почта", "Локальная оценка бюджета Postbox", &body),
        &cookies,
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

fn valid_csrf(state: &AuthState, headers: &HeaderMap, token: &str) -> bool {
    state.cookies.same_origin(headers) && state.cookies.valid_csrf(headers, token)
}

fn stop_local(control: Option<Extension<MailControl>>) {
    if let Some(Extension(control)) = control {
        // Poisoned local authority is already fail-closed; still perform
        // the durable emergency stop or configuration transaction.
        if control.stop().is_err() {
            tracing::warn!("System mail local authority unavailable");
        }
    }
}

fn rubles(form: &LimitsForm) -> Option<LimitChange> {
    Some(LimitChange {
        warning_1: form.warning_1.checked_mul(100)?,
        warning_2: form.warning_2.checked_mul(100)?,
        news_pause: form.news_pause.checked_mul(100)?,
        monthly_limit: form.monthly_limit.checked_mul(100)?,
    })
}

async fn save(
    State(state): State<AuthState>,
    headers: HeaderMap,
    control: Option<Extension<MailControl>>,
    Form(form): Form<LimitsForm>,
) -> Response {
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if !valid_csrf(&state, &headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(limits) = rubles(&form) else {
        return deny(StatusCode::BAD_REQUEST);
    };
    stop_local(control);
    match store(&state)
        .admin_save_limits(&digest, form.revision, limits)
        .await
    {
        Ok(()) => private_response(Redirect::to("/odmen/system-mail"), &[]),
        Err(error) => deny(status(error)),
    }
}

async fn stop(
    State(state): State<AuthState>,
    headers: HeaderMap,
    control: Option<Extension<MailControl>>,
    Form(form): Form<StopForm>,
) -> Response {
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if !valid_csrf(&state, &headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    stop_local(control);
    match store(&state).admin_stop(&digest).await {
        Ok(()) => private_response(Redirect::to("/odmen/system-mail"), &[]),
        Err(error) => deny(status(error)),
    }
}

async fn reconcile(
    State(state): State<AuthState>,
    headers: HeaderMap,
    control: Option<Extension<MailControl>>,
    Form(form): Form<ReconcileForm>,
) -> Response {
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if !valid_csrf(&state, &headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(request) = form.request() else {
        return deny(StatusCode::BAD_REQUEST);
    };
    stop_local(control);
    match store(&state)
        .admin_reconcile_month(&digest, form.revision, &request)
        .await
    {
        Ok(()) => private_response(Redirect::to("/odmen/system-mail"), &[]),
        Err(error) => deny(status(error)),
    }
}

async fn save_profile(
    State(state): State<AuthState>,
    headers: HeaderMap,
    control: Option<Extension<MailControl>>,
    Form(form): Form<ProfileForm>,
) -> Response {
    let password = Zeroizing::new(form.current_password);
    let secrets = ProfileSecrets::new(form.access, form.secret);
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if !valid_csrf(&state, &headers, &form.csrf_token) || !profile::secure_origin(&state) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Ok(secrets) = secrets else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let Ok(identity) = ProfileIdentity::new(form.identity_id, form.sender) else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let Some(next_revision) = form.revision.checked_add(1).filter(|value| *value > 1) else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let master = match profile::master_key() {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(value)) if value.administrator => value.actor.user_id.get(),
        Ok(_) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if password.is_empty() || password.len() > 512 {
        return deny(StatusCode::UNAUTHORIZED);
    }
    let attempt_key = session_token_digest(&format!("auth:postbox-stepup:{actor}"));
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
    let candidate = match master.seal(identity, next_revision, &secrets) {
        Ok(value) => value,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let proof = match state
        .accounts
        .issue_administrator_postbox_step_up(
            actor,
            &digest,
            &verified,
            form.revision,
            &candidate.intent_digest(),
        )
        .await
    {
        Ok(token) => Zeroizing::new(token),
        Err(StepUpIssueError::Forbidden) => return deny(StatusCode::NOT_FOUND),
        Err(StepUpIssueError::StepUpRequired) => return deny(StatusCode::UNAUTHORIZED),
        Err(StepUpIssueError::InvalidIntent) => return deny(StatusCode::BAD_REQUEST),
        Err(StepUpIssueError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    stop_local(control);
    match profile::store(&state)
        .admin_save_candidate(&digest, &proof, form.revision, &candidate)
        .await
    {
        Ok(()) => {
            let _ = state.accounts.clear_auth_attempt(&attempt_key).await;
            private_response(Redirect::to("/odmen/system-mail"), &[])
        }
        Err(error) => deny(profile::status(error)),
    }
}

async fn probe_profile(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ProbeForm>,
) -> Response {
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if !valid_csrf(&state, &headers, &form.csrf_token) || !profile::secure_origin(&state) {
        return deny(StatusCode::FORBIDDEN);
    }
    let master = match profile::master_key() {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(value)) if value.administrator => value.actor.user_id.get(),
        Ok(_) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let attempt_key = session_token_digest(&format!("mail:postbox-probe:{actor}"));
    match state
        .accounts
        .permit_auth_attempt(&attempt_key, 3, 60)
        .await
    {
        Ok(true) => {}
        Ok(false) => return deny(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let store = profile::store(&state);
    let candidate = match store.admin_load_candidate(&digest, form.revision).await {
        Ok(value) => value,
        Err(error) => return deny(profile::status(error)),
    };
    if let Err(error) = profile::probe_candidate(&master, &candidate).await {
        return deny(error);
    }
    if let Err(error) = store.admin_confirm_probe(&digest, &candidate).await {
        return deny(profile::status(error));
    }
    private_response(
        pages::layout(
            "Проверка Postbox",
            "SMTP-аутентификация",
            "<section class=\"section\"><div class=\"container\"><h1>Проверка пройдена</h1>\
             <p>TLS, аутентификация и NOOP успешны. Письмо не отправлялось. \
             Это не подтверждает право отправителя на конкретный адрес или доставляемость. \
             Отправка не включена.</p><p><a href=\"/odmen/system-mail\">Вернуться</a></p></div></section>",
        ),
        &[],
    )
}
