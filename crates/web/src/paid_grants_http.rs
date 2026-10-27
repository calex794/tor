//! S034 / C-06: authenticated, fail-closed HTTP grant for a purchased V1.
//! The browser supplies only the chapter and optional owned purchase id.
//! D authorizes; B resolves; G signs. No paid plaintext crosses this endpoint.
use crate::auth_cookies::{private_response, CookiePolicy};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::repositories::paid_storage::PgPaidStorage;
use tor_commerce::repositories::entitlements::PgDirectEntitlements;
use tor_contracts::paid_delivery::{Actor, DeliveryError, PositiveId, PrivateObjectDeliveryPort};
use tor_reading::services::paid_delivery::issue_paid_chapter_grant;

const WINDOW: Duration = Duration::from_secs(60);
const MAX_GRANTS_PER_ACTOR: u8 = 12;
const MAX_ACTORS: usize = 8192;
const MAX_CONCURRENT_SIGNINGS: usize = 4;

#[derive(Default)]
struct RateLimits {
    actors: HashMap<i64, (Instant, u8)>,
}

impl RateLimits {
    fn allow(&mut self, actor: i64, now: Instant) -> bool {
        if !self.actors.contains_key(&actor) && self.actors.len() >= MAX_ACTORS {
            self.actors
                .retain(|_, (started, _)| now.duration_since(*started) < WINDOW);
            if self.actors.len() >= MAX_ACTORS {
                return false;
            }
        }
        let entry = self.actors.entry(actor).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        if entry.1 >= MAX_GRANTS_PER_ACTOR {
            return false;
        }
        entry.1 += 1;
        true
    }
}

#[derive(Clone)]
struct GrantState {
    accounts: PgAccounts,
    entitlement: PgDirectEntitlements,
    versions: PgPaidStorage,
    storage: Arc<dyn PrivateObjectDeliveryPort>,
    cookies: CookiePolicy,
    rate_limits: Arc<Mutex<RateLimits>>,
    concurrent: Arc<Semaphore>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GrantBody {
    purchase_id: Option<String>,
}

fn positive(raw: &str) -> Option<PositiveId> {
    if raw.is_empty() || raw.len() > 19 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse::<i64>()
        .ok()
        .and_then(|value| PositiveId::new(value).ok())
}

fn status(error: DeliveryError) -> StatusCode {
    match error {
        DeliveryError::Unauthenticated => StatusCode::UNAUTHORIZED,
        DeliveryError::Denied => StatusCode::FORBIDDEN,
        DeliveryError::Hidden => StatusCode::NOT_FOUND,
        DeliveryError::Conflict => StatusCode::CONFLICT,
        DeliveryError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        DeliveryError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        DeliveryError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

fn deny(code: StatusCode) -> Response {
    private_response(code, &[])
}

async fn grant(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GrantBody>,
) -> Response {
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let actor = match state
        .accounts
        .active_session_user(&session_token_digest(&session))
        .await
    {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // Unlike ordinary HTML forms, this JSON grant requires a real same-origin
    // browser Origin header as well as the double-submit CSRF cookie.
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(chapter_id) = positive(&raw_chapter) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let purchase_id = match body.purchase_id.as_deref() {
        Some(value) => match positive(value) {
            Some(id) => Some(id),
            None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        },
        None => None,
    };
    let Ok(actor_id) = PositiveId::new(actor) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !state.rate_limits.lock().await.allow(actor, Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }

    let Ok(_permit) = state.concurrent.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };

    // C rechecks D after G signs, preventing newly refunded users from
    // receiving fresh bearer credentials. Already issued S3 URLs last <=90s.
    let result = issue_paid_chapter_grant(
        &state.entitlement,
        &state.versions,
        state.storage.as_ref(),
        Actor { user_id: actor_id },
        chapter_id,
        purchase_id,
    )
    .await;
    match result {
        Ok(grant) => {
            let mut response = private_response(Json(grant), &[]);
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
            response.headers_mut().insert(
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            );
            response
                .headers_mut()
                .insert("x-robots-tag", HeaderValue::from_static("noindex"));
            response
        }
        Err(error) => deny(status(error)),
    }
}

/// Mount this router ONLY after explicit operator enablement and valid
/// private S3 + separate content KEK configuration. No demo or default route.
pub(crate) fn router(
    accounts: PgAccounts,
    cookies: CookiePolicy,
    storage: Arc<dyn PrivateObjectDeliveryPort>,
) -> Router {
    let pool = accounts.pool().clone();
    let state = GrantState {
        accounts,
        entitlement: PgDirectEntitlements::new(pool.clone()),
        versions: PgPaidStorage::new(pool),
        storage,
        cookies,
        rate_limits: Arc::new(Mutex::new(RateLimits::default())),
        concurrent: Arc::new(Semaphore::new(MAX_CONCURRENT_SIGNINGS)),
    };
    Router::new()
        .route(
            "/api/v1/reader/chapters/{chapter_id}/delivery-grants",
            post(grant),
        )
        .layer(DefaultBodyLimit::max(512))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_grant_http_accepts_only_unsigned_positive_decimal_ids() {
        for bad in ["", "0", "-1", "+1", "01x", "18446744073709551615"] {
            assert!(positive(bad).is_none(), "{bad}");
        }
        assert_eq!(positive("42").unwrap().get(), 42);
        assert_eq!(status(DeliveryError::Denied), StatusCode::FORBIDDEN);
        assert_eq!(
            status(DeliveryError::Unavailable),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn paid_grant_http_refuses_client_chosen_versions_and_object_keys() {
        assert!(serde_json::from_str::<GrantBody>(r#"{}"#).is_ok());
        assert!(serde_json::from_str::<GrantBody>(r#"{"purchase_id":"31"}"#).is_ok());
        for forbidden in [
            r#"{"version_id":"12"}"#,
            r#"{"object_key":"private/chapter-versions/id.enc"}"#,
            r#"{"bucket":"tor-private-chapter-ci"}"#,
            r#"{"purchase_id":"1","user_id":"2"}"#,
        ] {
            assert!(serde_json::from_str::<GrantBody>(forbidden).is_err());
        }
    }

    #[test]
    fn paid_grants_are_bounded_per_authenticated_actor() {
        let now = Instant::now();
        let mut limits = RateLimits::default();
        for _ in 0..MAX_GRANTS_PER_ACTOR {
            assert!(limits.allow(10, now));
        }
        assert!(!limits.allow(10, now));
        assert!(limits.allow(11, now), "one actor may not throttle another");
        assert!(limits.allow(10, now + WINDOW), "new window resets quota");
    }
}
