//! Explicitly enabled service-to-service route; not a browser session API.
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use tor_mail::{
    domain::{
        postbox::relay::{RelayError, RelayVerifier},
        quota::identity::QuotaKeySigner,
        suppression::FeedbackDecision,
    },
    repositories::postbox_feedback::{PgPostboxFeedback, PostboxDecision},
};
use zeroize::Zeroizing;

const PATH: &str = "/internal/mail/postbox-feedback";
const MAX_BODY: usize = 65536;
const DATABASE_BUDGET: Duration = Duration::from_secs(5);

struct RelayState {
    auth: RelayVerifier,
    signer: QuotaKeySigner,
    store: PgPostboxFeedback,
    permits: Semaphore,
}

fn configure(
    store: PgPostboxFeedback,
    relay: &str,
    quota: &str,
    identity: &str,
) -> Option<Arc<RelayState>> {
    // A cloud relay must never possess the stable recipient/IP HMAC key.
    if relay.eq_ignore_ascii_case(quota) {
        return None;
    }
    Some(Arc::new(RelayState {
        auth: RelayVerifier::from_hex(relay, identity)?,
        signer: QuotaKeySigner::from_hex(quota)?,
        store,
        permits: Semaphore::new(4),
    }))
}

pub(super) fn router_from_env(accounts: &tor_accounts::repositories::PgAccounts, secure: bool) -> Router {
    if std::env::var("TOR_MAIL_POSTBOX_RELAY_ENABLED").as_deref() != Ok("1") {
        return Router::new();
    }
    let config = || {
        if !secure {
            return None;
        }
        let relay = Zeroizing::new(std::env::var("TOR_POSTBOX_RELAY_KEY").ok()?);
        let quota = Zeroizing::new(std::env::var("TOR_MAIL_QUOTA_KEY").ok()?);
        let identity = std::env::var("TOR_POSTBOX_IDENTITY_ID").ok()?;
        configure(
            PgPostboxFeedback::new(accounts.pool().clone()),
            relay.as_str(),
            quota.as_str(),
            &identity,
        )
    };
    match config() {
        Some(state) => router(state),
        None => {
            eprintln!("TOR_POSTBOX_RELAY_CONFIGURATION_UNAVAILABLE");
            Router::new()
        }
    }
}

fn router(state: Arc<RelayState>) -> Router {
    Router::new()
        .route(PATH, post(receive))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state)
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let values = headers.get_all(name);
    let mut iter = values.iter();
    let value = iter.next()?.to_str().ok()?;
    iter.next().is_none().then_some(value)
}

fn response(status: StatusCode) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")]).into_response()
}

async fn receive(
    State(state): State<Arc<RelayState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (Some(timestamp), Some(signature)) = (
        single_header(&headers, "x-tor-mail-timestamp"),
        single_header(&headers, "x-tor-mail-signature"),
    ) else {
        return response(StatusCode::UNAUTHORIZED);
    };
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return response(StatusCode::SERVICE_UNAVAILABLE);
    };
    let event = match state.auth.authenticate(timestamp, signature, &body, now.as_secs()) {
        Ok(Some(event)) => event,
        Ok(None) => return response(StatusCode::NO_CONTENT),
        Err(RelayError::Unauthorized) => return response(StatusCode::UNAUTHORIZED),
        Err(RelayError::InvalidPayload) => return response(StatusCode::BAD_REQUEST),
        Err(RelayError::ForeignIdentity) => return response(StatusCode::FORBIDDEN),
    };
    let Ok(_permit) = state.permits.try_acquire() else {
        return response(StatusCode::SERVICE_UNAVAILABLE);
    };
    let outcome = tokio::time::timeout(
        DATABASE_BUDGET,
        state.store.record_verified_event(&event, &state.signer),
    )
    .await;
    // The relay acknowledges only 204. Retry on missing correlation (the
    // receipt can arrive later), overload or ambiguous DB acknowledgement.
    let status = match outcome {
        Ok(Ok(PostboxDecision::Duplicate))
        | Ok(Ok(PostboxDecision::Feedback(
            FeedbackDecision::Applied | FeedbackDecision::Unchanged | FeedbackDecision::Ignored,
        ))) => StatusCode::NO_CONTENT,
        Ok(Ok(PostboxDecision::InvalidRecipient)) => StatusCode::UNPROCESSABLE_ENTITY,
        Ok(Ok(PostboxDecision::Uncorrelated | PostboxDecision::Feedback(FeedbackDecision::Uncorrelated))) => {
            StatusCode::CONFLICT
        }
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    response(status)
}

#[cfg(test)]
mod tests;
