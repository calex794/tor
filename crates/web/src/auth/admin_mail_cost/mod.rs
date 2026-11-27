//! External system mail cost page; never touches the internal message system.
use super::AuthState;
use crate::{
    auth_cookies::{deny, private_response},
    pages,
};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tor_accounts::services::security::session_token_digest;
use tor_mail::repositories::cost::{
    admin::{CostAdminError, CostAdminView, LimitChange, MonthReconciliation},
    PgMailCost,
};

mod view;

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/odmen/system-mail", get(page).post(save))
        .route("/odmen/system-mail/stop", post(stop))
        .route("/odmen/system-mail/reconcile", post(reconcile))
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

async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    let current = match store(&state).admin_view(&digest).await {
        Ok(value) => value,
        Err(error) => return deny(status(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        pages::layout(
            "Системная почта",
            "Локальная оценка бюджета Postbox",
            &view::body(&current, &csrf),
        ),
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
    Form(form): Form<StopForm>,
) -> Response {
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    if !valid_csrf(&state, &headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    match store(&state).admin_stop(&digest).await {
        Ok(()) => private_response(Redirect::to("/odmen/system-mail"), &[]),
        Err(error) => deny(status(error)),
    }
}

async fn reconcile(
    State(state): State<AuthState>,
    headers: HeaderMap,
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
    match store(&state)
        .admin_reconcile_month(&digest, form.revision, &request)
        .await
    {
        Ok(()) => private_response(Redirect::to("/odmen/system-mail"), &[]),
        Err(error) => deny(status(error)),
    }
}
