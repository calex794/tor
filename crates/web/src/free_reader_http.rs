//! S349: short free chapter S3 GET grant after current C/PG visibility checks.
//! The scripted reader downloads directly from S3. The no-script document
//! returns verified text so TOR can record the exact edition served.
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    paid_grants_http::valid_reader_s3_origin,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_commerce::repositories::layer_entitlements::PgLayerChapterEntitlements;
use tor_contracts::paid_delivery::SecretValue;
use tor_reading::repositories::{
    free_open_capture::PgFreeOpenCapture,
    free_s3_ref::{FreeS3Ref, PgFreeS3Ref},
};
use tor_storage::{
    repositories::free_chapter_s3::S3FreeChapterStore,
    services::free_chapter::{FreeChapterObject, FreeChapterStore},
};

mod layer_archive;
mod verified_grant;

#[derive(Clone)]
struct FreeGrantState {
    accounts: PgAccounts,
    layer: PgLayerChapterEntitlements,
    references: PgFreeS3Ref,
    opened: PgFreeOpenCapture,
    cookies: CookiePolicy,
    store: S3FreeChapterStore,
    source_slots: Arc<Semaphore>,
    bucket: String,
    s3_origin: String,
}

#[derive(Serialize)]
struct FreeGrant {
    schema: &'static str,
    chapter_id: String,
    version_id: String,
    url: SecretValue,
    sha256: String,
    size_bytes: u64,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Lookup {
    mode: Option<String>,
    version_id: Option<i64>,
    format: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmedOpen {
    version_id: String,
    sha256: String,
}

/// Public route, not an anonymous object listing. The reader's session is
/// optional only for BASIC public chapters.
pub fn router(
    accounts: PgAccounts,
    secure: bool,
    public_origin: String,
    store: S3FreeChapterStore,
    bucket: String,
    s3_origin: String,
) -> Result<Router, &'static str> {
    if !valid_reader_s3_origin(&s3_origin) {
        return Err("Invalid TOR_FREE_S3_PUBLIC_ORIGIN");
    }
    // A private historical FREE route is opt-in, independently from the
    // ordinary public S349 grant and the paid Layer reader.
    let archive_enabled = match std::env::var("TOR_LAYER_FREE_ARCHIVE_ENABLED") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(std::env::VarError::NotPresent) => false,
        _ => return Err("TOR_LAYER_FREE_ARCHIVE_ENABLED must be 0 or 1"),
    };
    let state = FreeGrantState {
        layer: PgLayerChapterEntitlements::new(accounts.pool().clone()),
        references: PgFreeS3Ref::new(accounts.pool().clone()),
        opened: PgFreeOpenCapture::new(accounts.pool().clone()),
        accounts,
        cookies: CookiePolicy {
            secure,
            origin: public_origin,
        },
        store,
        source_slots: Arc::new(Semaphore::new(16)),
        bucket,
        s3_origin,
    };
    let mut routes = Router::new()
        .route("/api/v1/chapters/{id}/free-grant", get(grant))
        .route("/api/v1/chapters/{id}/free-open", post(confirm_open));
    if archive_enabled {
        routes = routes
            .route("/read/layer/free/{id}", get(layer_archive::page))
            .route(
                "/api/v1/reader/layer/free/{id}/grant",
                post(layer_archive::grant),
            )
            .route(
                "/assets/newboo-layer-free-reader.mjs",
                get(layer_archive::script),
            );
    }
    Ok(routes.with_state(state))
}

async fn grant(
    State(state): State<FreeGrantState>,
    Path(id): Path<i64>,
    Query(query): Query<Lookup>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    if id <= 0 {
        return private_response(StatusCode::NOT_FOUND, &[]);
    }
    let digest = state
        .cookies
        .session_token(&headers)
        .map(|token| session_token_digest(&token));
    let saved = match query.mode.as_deref() {
        None => false,
        Some("saved") => true,
        _ => return private_response(StatusCode::BAD_REQUEST, &[]),
    };
    if saved && query.version_id.is_none_or(|id| id <= 0)
        || query.format.as_deref().is_some_and(|s| s != "source")
    {
        return private_response(StatusCode::BAD_REQUEST, &[]);
    }
    let resolve = || async {
        let result = if saved {
            state
                .references
                .for_saved_session(id, query.version_id.unwrap_or(0), digest.as_ref())
                .await
        } else {
            state.references.for_session(id, digest.as_ref()).await
        };
        result.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
    };
    if query.format.as_deref() == Some("source") {
        if !state.cookies.same_origin(&headers)
            || crate::public_reader_http::is_speculative_navigation(&headers)
        {
            return private_response(StatusCode::FORBIDDEN, &[]);
        }
        if headers.contains_key(header::RANGE) || headers.contains_key(header::IF_RANGE) {
            return private_response(StatusCode::BAD_REQUEST, &[]);
        }
        let Ok(_slot) = state.source_slots.try_acquire() else {
            return private_response(StatusCode::TOO_MANY_REQUESTS, &[]);
        };
        let (record, bytes) =
            match verified_grant::issue(resolve, |record| read_source(&state, record)).await {
                Ok(source) => source,
                Err(status) => return private_response(status, &[]),
            };
        if !saved && method == Method::GET {
            record_served_open(&state.accounts, &state.opened, digest.as_ref(), &record).await;
        }
        return free_source_response(bytes);
    }
    // Recheck after signing to prevent a stale capability after a hide, age
    // revocation or publication change during the storage call.
    let (record, signed) = match verified_grant::issue(resolve, |record| sign(&state, record)).await
    {
        Ok(grant) => grant,
        Err(status) => return private_response(status, &[]),
    };
    // A verified signed SINGLE may finish loading after V2 is published.
    // Save an actor/version/SHA permit before releasing the signed URL.
    if !saved && method == Method::GET {
        if let Some(digest) = digest.as_ref() {
            let actor = match state.accounts.active_session_user(digest).await {
                Ok(value) => value,
                Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
            };
            if let Some(actor) = actor {
                if state
                    .opened
                    .issue_intent(actor, id, record.version_id, record.sha256)
                    .await
                    .is_err()
                {
                    return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
                }
            }
        }
    }
    let sha256 = record
        .sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let response = Json(FreeGrant {
        schema: "tor.free-s3-grant.v1",
        chapter_id: record.chapter_id.to_string(),
        version_id: record.version_id.to_string(),
        url: signed,
        sha256,
        size_bytes: record.size_bytes,
    });
    free_response(private_response(response, &[]))
}

pub(crate) fn free_source_response(bytes: Vec<u8>) -> Response {
    free_response(private_response(
        ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], bytes),
        &[],
    ))
}

fn free_response(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(crate) async fn record_served_open(
    accounts: &PgAccounts,
    opened: &PgFreeOpenCapture,
    digest: Option<&[u8; 32]>,
    edition: &FreeS3Ref,
) {
    let Some(digest) = digest else { return };
    let result = match accounts.active_session_user(digest).await {
        Ok(Some(actor)) => {
            opened
                .record(actor, edition.chapter_id, edition.version_id)
                .await
        }
        Ok(None) => Ok(false),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        tracing::warn!(?error, "exact FREE source open not recorded");
    }
}

/// A FREE S3 shell cannot prove bytes were displayed. The verified browser
/// reports only after a successful GET, byte count, SHA-256 and render.
/// The server rechecks exact active B/C metadata, session, CSRF and age.
/// This never grants access to arbitrary historical revisions.
async fn confirm_open(
    State(state): State<FreeGrantState>,
    Path(chapter): Path<i64>,
    headers: HeaderMap,
    Json(confirm): Json<ConfirmedOpen>,
) -> Response {
    if chapter <= 0
        || !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|token| state.cookies.valid_csrf(&headers, token))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let version: i64 = match confirm.version_id.parse() {
        Ok(id) if id > 0 => id,
        _ => return private_response(StatusCode::BAD_REQUEST, &[]),
    };
    if confirm.sha256.len() != 64
        || !confirm.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || confirm.sha256.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return private_response(StatusCode::BAD_REQUEST, &[]);
    }
    let Some(session) = state.cookies.session_token(&headers) else {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    };
    let digest = session_token_digest(&session);
    let actor = match state.accounts.active_session_user(&digest).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(StatusCode::UNAUTHORIZED, &[]),
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    // The previous grant is authoritative for a delayed V1 receipt.
    // Checking current publication instead would discard a real V1 open.
    match state
        .opened
        .record_confirmed(actor, chapter, version, &confirm.sha256)
        .await
    {
        Ok(true) => private_response(StatusCode::NO_CONTENT, &[]),
        Ok(false) => private_response(StatusCode::CONFLICT, &[]),
        Err(_) => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

fn trusted_object(
    state: &FreeGrantState,
    record: &FreeS3Ref,
) -> Result<FreeChapterObject, StatusCode> {
    if record.bucket != state.bucket {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    FreeChapterObject::from_metadata(
        record.chapter_id,
        record.version_id,
        record.sha256,
        record.size_bytes,
    )
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

async fn sign(state: &FreeGrantState, record: FreeS3Ref) -> Result<SecretValue, StatusCode> {
    let object = trusted_object(state, &record)?;
    let signed = state
        .store
        .presign_verified(&object, Duration::from_secs(90))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Never return a signed internal/private endpoint to the browser.
    let raw = serde_json::to_value(&signed).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let url = raw.as_str().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let parsed = url
        .parse::<Uri>()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let actual = parsed
        .authority()
        .zip(parsed.scheme_str())
        .map(|(authority, scheme)| format!("{scheme}://{authority}"));
    if actual.as_deref() != Some(state.s3_origin.as_str()) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if !state
        .references
        .record_single_delivery(&record)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(signed)
}

async fn read_source(state: &FreeGrantState, record: FreeS3Ref) -> Result<Vec<u8>, StatusCode> {
    let object = trusted_object(state, &record)?;
    tokio::time::timeout(Duration::from_secs(15), state.store.get_verified(&object))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
