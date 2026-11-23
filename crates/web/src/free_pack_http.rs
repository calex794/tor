//! DROP-06a: independently mountable, private server-side Range gateway.
//! Mounted by the runtime only with explicit DROP-06b operator opt-in.
//! The original S349 signed SINGLE grant is unchanged when disabled.
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    free_pack_gateway::{read_authorized_active, FreePackReadMode},
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::repositories::free_pack_attach::PgFreePackCatalog;
use tor_commerce::repositories::layer_entitlements::PgLayerChapterEntitlements;
use tor_reading::repositories::{
    free_open_capture::PgFreeOpenCapture,
    free_s3_ref::{FreeS3Ref, PgFreeS3Ref},
};
use tor_storage::{
    domain::provider::{valid_private_bucket, StorageError},
    repositories::free_pack_range_s3::S3PackFrameStore,
};

mod layer_archive;

#[derive(Clone)]
struct PackHttpState {
    accounts: PgAccounts,
    layer: PgLayerChapterEntitlements,
    reference: PgFreeS3Ref,
    opened: PgFreeOpenCapture,
    placement: PgFreePackCatalog,
    frame_store: S3PackFrameStore,
    cookies: CookiePolicy,
    bucket: String,
    inflight: Arc<Semaphore>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Lookup {
    mode: Option<String>,
    version_id: Option<i64>,
}

/// Separate router: main.rs mounts it only after explicit operator opt-in.
pub fn router(
    accounts: PgAccounts,
    secure: bool,
    origin: String,
    s3: aws_sdk_s3::Client,
    bucket: String,
) -> Result<Router, &'static str> {
    if !valid_private_bucket(&bucket) {
        return Err("Invalid private FREE PACK bucket");
    }
    // This historical PACK route is an independent, disabled-by-default
    // capability. The existing public/saved FREE PACK gateway is unaffected.
    let layer_archive_enabled = match std::env::var("TOR_LAYER_FREE_ARCHIVE_ENABLED") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(std::env::VarError::NotPresent) => false,
        _ => return Err("TOR_LAYER_FREE_ARCHIVE_ENABLED must be 0 or 1"),
    };
    let pool = accounts.pool().clone();
    let mut routes = Router::new()
        .route("/api/v1/chapters/{id}/free-pack-source", get(source))
        .route("/api/v1/chapters/{id}/free-source-plan", get(plan))
        .route("/api/v1/chapters/{id}/free-source", get(no_js_source));
    if layer_archive_enabled {
        routes = routes.route(
            "/api/v1/reader/layer/free/{id}/pack",
            post(layer_archive::read_pack),
        );
    }
    Ok(routes.with_state(PackHttpState {
        layer: PgLayerChapterEntitlements::new(pool.clone()),
        reference: PgFreeS3Ref::new(pool.clone()),
        opened: PgFreeOpenCapture::new(pool.clone()),
        placement: PgFreePackCatalog::new(pool),
        accounts,
        frame_store: S3PackFrameStore::new(s3),
        cookies: CookiePolicy { secure, origin },
        bucket,
        inflight: Arc::new(Semaphore::new(16)),
    }))
}

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
}

#[derive(Serialize)]
struct SourcePlan {
    schema: &'static str,
    chapter_id: String,
    version_id: String,
    transport: &'static str,
    sha256: String,
    size_bytes: u64,
}

/// C selects the exact already-authorized edition before advertising any
/// transport. An active PACK without B provenance must fail closed, not
/// silently downgrade to an obsolete SINGLE capability.
/// Resolve a transport using live publication/pin rights and the active
/// verified physical generation. This shared decision is rechecked by both
/// the PACK gateway and the existing SINGLE grant before any bytes are read.
async fn selected_source(
    state: &PackHttpState,
    chapter: i64,
    mode: FreePackReadMode,
    digest: Option<&[u8; 32]>,
) -> Result<(FreeS3Ref, &'static str), StatusCode> {
    let active = match mode {
        FreePackReadMode::Published => {
            state
                .reference
                .for_active_pack_session(chapter, digest)
                .await
        }
        FreePackReadMode::Saved { version_id } => {
            state
                .reference
                .for_saved_active_pack_session(chapter, version_id, digest)
                .await
        }
    };
    let active = match active {
        Ok(record) => record,
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (edition, transport) = if let Some(record) = active {
        let proven = state
            .placement
            .active_verified_free_range(
                record.chapter_id,
                record.version_id,
                record.sha256,
                record.size_bytes,
                &record.bucket,
            )
            .await;
        match proven {
            Ok(Some(_)) => (record, "gateway"),
            Ok(None) => return Err(StatusCode::SERVICE_UNAVAILABLE),
            Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        let single = match mode {
            FreePackReadMode::Published => state.reference.for_session(chapter, digest).await,
            FreePackReadMode::Saved { version_id } => {
                state
                    .reference
                    .for_saved_session(chapter, version_id, digest)
                    .await
            }
        };
        match single {
            Ok(Some(record)) => (record, "signed-single"),
            Ok(None) => return Err(StatusCode::NOT_FOUND),
            Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
        }
    };
    if edition.bucket != state.bucket {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok((edition, transport))
}

/// Non-script fallback: one secure same-origin link for both transport types.
/// PACK returns exactly one reauthorized frame. SINGLE redirects to the old
/// independently reauthorized grant, which issues a short-lived signed GET.
async fn no_js_source(
    State(state): State<PackHttpState>,
    Path(chapter): Path<i64>,
    Query(query): Query<Lookup>,
    headers: HeaderMap,
) -> Response {
    if chapter <= 0 || headers.contains_key(header::RANGE) || headers.contains_key("if-range") {
        return deny(StatusCode::BAD_REQUEST);
    }
    if headers.get("sec-fetch-site").and_then(|s| s.to_str().ok()) == Some("cross-site")
        || headers
            .get(header::ORIGIN)
            .is_some_and(|s| s.to_str().ok() != Some(state.cookies.origin.as_str()))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let mode = match (query.mode.as_deref(), query.version_id) {
        (None, None) => FreePackReadMode::Published,
        (Some("saved"), Some(version_id)) if version_id > 0 => {
            FreePackReadMode::Saved { version_id }
        }
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let digest = state
        .cookies
        .session_token(&headers)
        .map(|token| session_token_digest(&token));
    let (_, transport) = match selected_source(&state, chapter, mode, digest.as_ref()).await {
        Ok(selection) => selection,
        Err(status) => return deny(status),
    };
    if transport == "gateway" {
        // source() rechecks C/B before and after private S3 Range I/O.
        return source(State(state), Path(chapter), Query(query), headers).await;
    }
    // Redirects always target a same-origin literal route, not arbitrary S3.
    // The S349 grant independently checks publication, pin and age again.
    let target = match mode {
        FreePackReadMode::Published =>
            format!("/api/v1/chapters/{chapter}/free-grant?format=source"),
        FreePackReadMode::Saved { version_id } =>
            format!("/api/v1/chapters/{chapter}/free-grant?mode=saved&version_id={version_id}&format=source"),
    };
    let mut response = private_response(axum::response::Redirect::to(&target), &[]);
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

async fn plan(
    State(state): State<PackHttpState>,
    Path(chapter): Path<i64>,
    Query(query): Query<Lookup>,
    headers: HeaderMap,
) -> Response {
    if chapter <= 0 || headers.contains_key(header::RANGE) || headers.contains_key("if-range") {
        return deny(StatusCode::BAD_REQUEST);
    }
    if headers.get("sec-fetch-site").and_then(|s| s.to_str().ok()) == Some("cross-site")
        || headers
            .get(header::ORIGIN)
            .is_some_and(|s| s.to_str().ok() != Some(state.cookies.origin.as_str()))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let mode = match (query.mode.as_deref(), query.version_id) {
        (None, None) => FreePackReadMode::Published,
        (Some("saved"), Some(version)) if version > 0 => FreePackReadMode::Saved {
            version_id: version,
        },
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let digest = state
        .cookies
        .session_token(&headers)
        .map(|token| session_token_digest(&token));
    let published = matches!(mode, FreePackReadMode::Published);
    let (edition, transport) = match selected_source(&state, chapter, mode, digest.as_ref()).await {
        Ok(selection) => selection,
        Err(status) => return deny(status),
    };
    // The server advertises the verified exact PACK edition before download.
    // Keep a short-lived actor-specific permit so a concurrent author edit
    // cannot erase a later, valid V1 browser acknowledgement.
    if published {
        if let Some(digest) = digest.as_ref() {
            let actor = match state.accounts.active_session_user(digest).await {
                Ok(value) => value,
                Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
            };
            if let Some(actor) = actor {
                if state.opened.issue_intent(actor, chapter, edition.version_id, edition.sha256)
                    .await.is_err() {
                    return deny(StatusCode::SERVICE_UNAVAILABLE);
                }
            }
        }
    }
    let sha = edition
        .sha256
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut response = private_response(
        Json(SourcePlan {
            schema: "tor.free-source-plan.v1",
            chapter_id: edition.chapter_id.to_string(),
            version_id: edition.version_id.to_string(),
            transport,
            sha256: sha,
            size_bytes: edition.size_bytes,
        }),
        &[],
    );
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

async fn source(
    State(state): State<PackHttpState>,
    Path(chapter): Path<i64>,
    Query(query): Query<Lookup>,
    headers: HeaderMap,
) -> Response {
    if chapter <= 0 || headers.contains_key(header::RANGE) || headers.contains_key("if-range") {
        return deny(StatusCode::BAD_REQUEST);
    }
    // No cross-origin content grants; the session cookie is SameSite=Strict.
    // Anonymous public/basic access still returns no CORS permission.
    if headers.get("sec-fetch-site").and_then(|s| s.to_str().ok()) == Some("cross-site")
        || headers
            .get(header::ORIGIN)
            .is_some_and(|s| s.to_str().ok() != Some(state.cookies.origin.as_str()))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let mode = match (query.mode.as_deref(), query.version_id) {
        (None, None) => FreePackReadMode::Published,
        (Some("saved"), Some(version)) if version > 0 => FreePackReadMode::Saved {
            version_id: version,
        },
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    // Bound simultaneous S3 Range/decode work even when requests are anonymous.
    let _slot = match state.inflight.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return deny(StatusCode::TOO_MANY_REQUESTS),
    };
    let digest = state
        .cookies
        .session_token(&headers)
        .map(|token| session_token_digest(&token));
    let (edition, bytes) = match read_authorized_active(
        &state.reference,
        &state.placement,
        &state.frame_store,
        chapter,
        digest.as_ref(),
        mode,
    )
    .await
    {
        Ok(ok) => ok,
        Err(StorageError::Invalid) => return deny(StatusCode::NOT_FOUND),
        Err(StorageError::Conflict) => return deny(StatusCode::CONFLICT),
        Err(StorageError::Corrupt | StorageError::Unavailable) => {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    if edition.bucket != state.bucket {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut response = private_response(
        ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], bytes),
        &[],
    );
    let sha = edition
        .sha256
        .iter()
        .map(|octet| format!("{octet:02x}"))
        .collect::<String>();
    for (key, value) in [
        ("x-tor-free-version-id", edition.version_id.to_string()),
        ("x-tor-free-sha256", sha),
        ("x-tor-free-size", edition.size_bytes.to_string()),
    ] {
        let Ok(header) = HeaderValue::from_str(&value) else {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        };
        response.headers_mut().insert(key, header);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_unknown_bucket_or_cross_site_range_can_be_published_by_constructor() {
        assert!(!valid_private_bucket("public-free"));
        assert!(!valid_private_bucket("localhost/../bucket"));
        assert!(valid_private_bucket("tor-free-pack-ci"));
    }
}
