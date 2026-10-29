//! S034 / C-06: authenticated, fail-closed HTTP grant for a purchased V1.
//! The browser supplies only the chapter and optional owned purchase id.
//! D authorizes; B resolves; G signs. No paid plaintext crosses this endpoint.
mod asset_grants;
mod exposure;
mod layer_reader;

use crate::{
    auth_cookies::{private_response, CookiePolicy},
    pages,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::Response,
    routing::{get, post},
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
use tor_books::repositories::{paid_assets::PgPaidAssets, paid_storage::PgPaidStorage};
use tor_commerce::repositories::{
    entitlements::PgDirectEntitlements,
    direct_exposure::PgDirectFirstExposure,
    layer_entitlements::{PgLayerChapterEntitlements, PgReaderEntitlements},
    layer_exposure::{LayerGrantKind, PgLayerFirstExposure},
};
use tor_contracts::paid_delivery::{
    Actor, DeliveryError, EntitlementDecision, EntitlementPort, PositiveId,
    PrivateObjectDeliveryPort,
};
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
    entitlement: PgReaderEntitlements,
    layer: PgLayerChapterEntitlements,
    direct: PgDirectEntitlements,
    first_direct_grant: PgDirectFirstExposure,
    first_layer_grant: PgLayerFirstExposure,
    versions: PgPaidStorage,
    assets: PgPaidAssets,
    storage: Arc<dyn PrivateObjectDeliveryPort>,
    cookies: CookiePolicy,
    rate_limits: Arc<Mutex<RateLimits>>,
    concurrent: Arc<Semaphore>,
    reader_s3_origin: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReaderQuery {
    purchase_id: String,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PurchaseListQuery {
    after: Option<String>,
}

/// Only operator-owned HTTPS origins can reach CSP/connect-src. Loopback HTTP
/// is allowed exclusively for isolated browser tests and local development.
pub(crate) fn valid_reader_s3_origin(origin: &str) -> bool {
    if origin.is_empty() || origin.contains('#') || origin.contains('?') || origin.ends_with('/') {
        return false;
    }
    let Ok(uri) = origin.parse::<Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    let host = uri.host().unwrap_or("");
    let secure = uri.scheme_str() == Some("https");
    let local_http =
        uri.scheme_str() == Some("http") && (host == "127.0.0.1" || host == "localhost");
    (secure || local_http)
        && !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        && !authority.as_str().contains('@')
        && uri.path_and_query().is_none_or(|path| path.as_str() == "/")
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GrantSource {
    Layer,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GrantBody {
    purchase_id: Option<String>,
    source: Option<GrantSource>,
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

    // The requested source selects a D authority; it is not proof of rights.
    // An explicit purchase never falls through to a Layer term.
    if body.source.is_some() && purchase_id.is_some() {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let authority: &dyn EntitlementPort = match body.source {
        Some(GrantSource::Layer) => &state.layer,
        None => &state.entitlement,
    };
    // C rechecks the same D source after G signs. Existing S3 bearer
    // credentials remain usable only until their short expiry.
    let result = issue_paid_chapter_grant(
        authority,
        &state.versions,
        state.storage.as_ref(),
        Actor { user_id: actor_id },
        chapter_id,
        purchase_id,
    )
    .await;
    match result {
        Ok(grant) => {
            // JSON must be built before exposure is written; failed G signing,
            // edition mismatch and metadata shells never record an open.
            let version_id = grant.version_id.clone();
            let mut response = private_response(Json(grant), &[]);
            if let Err(error) = exposure::record_paid_grant_exposure(
                &state,
                actor_id,
                chapter_id,
                purchase_id,
                body.source,
                &version_id,
                LayerGrantKind::Chapter,
            )
            .await
            {
                return deny(status(error));
            }
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

/// The page contains only a generic reader shell. D is checked here and
/// again in the POST; the browser never receives a bearer grant in HTML.
async fn paid_reader(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    Query(query): Query<ReaderQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(s3_origin) = state.reader_s3_origin.as_deref() else {
        return deny(StatusCode::NOT_FOUND);
    };
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
    let (Some(chapter_id), Some(purchase_id), Ok(actor_id)) = (
        positive(&raw_chapter),
        positive(&query.purchase_id),
        PositiveId::new(actor),
    ) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let pinned = match state
        .direct
        .resolve_entitlement(Actor { user_id: actor_id }, chapter_id, Some(purchase_id))
        .await
    {
        Ok(EntitlementDecision::Allowed { pinned }) if pinned.chapter_id == chapter_id => pinned,
        Ok(EntitlementDecision::Allowed { .. }) => return deny(StatusCode::CONFLICT),
        Ok(EntitlementDecision::Denied { reason }) => {
            return deny(status(match reason {
                tor_contracts::paid_delivery::DenialReason::Hidden => DeliveryError::Hidden,
                tor_contracts::paid_delivery::DenialReason::Unknown => DeliveryError::Unavailable,
                _ => DeliveryError::Denied,
            }));
        }
        Err(error) => return deny(status(error)),
    };
    // Metadata is also scoped by D's canonical SQL policy. A concurrent
    // refund/hold between checks must not expose stale paid-page details.
    let details = match state
        .direct
        .readable_purchase_detail(actor_id, chapter_id, purchase_id)
        .await
    {
        Ok(Some(details)) if details.version_id == pinned.version_id.get() => details,
        Ok(Some(_)) => return deny(StatusCode::CONFLICT),
        Ok(None) => return deny(StatusCode::FORBIDDEN),
        Err(error) => return deny(status(error)),
    };
    let toc = match state
        .direct
        .mixed_book_toc(actor_id, chapter_id, purchase_id)
        .await
    {
        Ok(toc) => toc,
        Err(error) => return deny(status(error)),
    };
    // Never allow free/public metadata to satisfy ownership. D must still
    // confirm that the original pinned paid version is readable.
    if !toc.chapters.iter().any(|row| {
        row.chapter_id == chapter_id.get()
            && matches!(
                row.access,
                tor_commerce::repositories::entitlements::MixedTocAccess::Purchased {
                    purchase_id: id,
                    version_id: version,
                    ..
                } if id == purchase_id.get() && version == pinned.version_id.get()
            )
    }) {
        return deny(StatusCode::FORBIDDEN);
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        pages::paid_reader_shell(&details, &toc, &csrf, s3_origin),
        &cookies,
    );
    let csp = format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; \
         connect-src 'self' {s3_origin}; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    );
    let Ok(value) = HeaderValue::from_str(&csp) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    response
        .headers_mut()
        .insert("content-security-policy", value);
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
        .headers_mut()
        .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}

/// The only entry point to the paid reader. It never derives a grant from
/// history/progress and shows only purchases still approved by canonical D.
async fn purchased_library(
    State(state): State<GrantState>,
    Query(query): Query<PurchaseListQuery>,
    headers: HeaderMap,
) -> Response {
    if state.reader_s3_origin.is_none() {
        return deny(StatusCode::NOT_FOUND);
    }
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
    let Ok(actor) = PositiveId::new(actor) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    let after = match query.after {
        Some(raw) => match positive(&raw) {
            Some(id) => Some(id),
            None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        },
        None => None,
    };
    let owned = match state.direct.readable_purchases(actor, after).await {
        Ok(owned) => owned,
        Err(error) => return deny(status(error)),
    };
    let mut response = private_response(pages::purchased_chapters_page(&owned), &[]);
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
        .headers_mut()
        .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}

/// Mount this router ONLY after explicit operator enablement and valid
/// private S3 + separate content KEK configuration. No demo or default route.
pub(crate) fn router(
    accounts: PgAccounts,
    cookies: CookiePolicy,
    storage: Arc<dyn PrivateObjectDeliveryPort>,
) -> Router {
    router_inner(accounts, cookies, storage, None)
}

/// Browser route is opt-in separately from the API grant; tests of the grant
/// alone remain unchanged and cannot accidentally expose a reader page.
pub(crate) fn router_with_reader(
    accounts: PgAccounts,
    cookies: CookiePolicy,
    storage: Arc<dyn PrivateObjectDeliveryPort>,
    reader_s3_origin: String,
) -> Result<Router, &'static str> {
    if !valid_reader_s3_origin(&reader_s3_origin) {
        return Err("private paid S3 origin must be HTTPS or localhost");
    }
    Ok(router_inner(
        accounts,
        cookies,
        storage,
        Some(reader_s3_origin),
    ))
}

fn router_inner(
    accounts: PgAccounts,
    cookies: CookiePolicy,
    storage: Arc<dyn PrivateObjectDeliveryPort>,
    reader_s3_origin: Option<String>,
) -> Router {
    let pool = accounts.pool().clone();
    let state = GrantState {
        accounts,
        entitlement: PgReaderEntitlements::new(pool.clone()),
        layer: PgLayerChapterEntitlements::new(pool.clone()),
        direct: PgDirectEntitlements::new(pool.clone()),
        first_direct_grant: PgDirectFirstExposure::new(pool.clone()),
        first_layer_grant: PgLayerFirstExposure::new(pool.clone()),
        versions: PgPaidStorage::new(pool.clone()),
        assets: PgPaidAssets::new(pool),
        storage,
        cookies,
        rate_limits: Arc::new(Mutex::new(RateLimits::default())),
        concurrent: Arc::new(Semaphore::new(MAX_CONCURRENT_SIGNINGS)),
        reader_s3_origin,
    };
    let mut app = Router::new()
        .route(
            "/api/v1/reader/chapters/{chapter_id}/delivery-grants",
            post(grant),
        )
        .route(
            "/api/v1/reader/chapters/{chapter_id}/assets/{object_id}/delivery-grants",
            post(asset_grants::grant_asset),
        );
    if state.reader_s3_origin.is_some() {
        app = app
            .route("/read/layer/{chapter_id}", get(layer_reader::layer_reader))
            .route("/read/paid/{chapter_id}", get(paid_reader))
            .route("/me/purchases", get(purchased_library));
    }
    app.layer(DefaultBodyLimit::max(512)).with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h_browser_s3_origin_is_server_owned_and_no_redirect_target() {
        for accepted in [
            "https://objects.example.test",
            "https://objects.example.test:443",
            "http://127.0.0.1:9000",
        ] {
            assert!(valid_reader_s3_origin(accepted), "{accepted}");
        }
        for denied in [
            "",
            "http://external.example",
            "https://host.example/path",
            "https://host.example/",
            "https://user@host.example",
            "https://host.example/?q=1",
            "javascript:alert(1)",
        ] {
            assert!(!valid_reader_s3_origin(denied), "{denied}");
        }
    }

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
