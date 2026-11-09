//! S349: short free chapter S3 GET grant after current C/PG visibility checks.
//! The browser downloads Markdown from S3, never through the TOR HTTP server.
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    paid_grants_http::valid_reader_s3_origin,
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::Response,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::time::Duration;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_contracts::paid_delivery::SecretValue;
use tor_reading::repositories::free_s3_ref::PgFreeS3Ref;
use tor_storage::{
    repositories::free_chapter_s3::S3FreeChapterStore,
    services::free_chapter::{FreeChapterObject, FreeChapterStore},
};

#[derive(Clone)]
struct FreeGrantState {
    references: PgFreeS3Ref,
    cookies: CookiePolicy,
    store: S3FreeChapterStore,
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
    let state = FreeGrantState {
        references: PgFreeS3Ref::new(accounts.pool().clone()),
        cookies: CookiePolicy {
            secure,
            origin: public_origin,
        },
        store,
        bucket,
        s3_origin,
    };
    Ok(Router::new()
        .route("/api/v1/chapters/{id}/free-grant", get(grant))
        .with_state(state))
}

async fn grant(
    State(state): State<FreeGrantState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if id <= 0 {
        return private_response(StatusCode::NOT_FOUND, &[]);
    }
    let digest = state
        .cookies
        .session_token(&headers)
        .map(|token| session_token_digest(&token));
    let record = match state.references.for_session(id, digest.as_ref()).await {
        Ok(Some(record)) => record,
        Ok(None) => return private_response(StatusCode::NOT_FOUND, &[]),
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    if record.bucket != state.bucket {
        return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
    }
    let object = match FreeChapterObject::from_metadata(
        record.chapter_id,
        record.version_id,
        record.sha256,
        record.size_bytes,
    ) {
        Ok(object) => object,
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let signed = match state
        .store
        .presign_verified(&object, Duration::from_secs(90))
        .await
    {
        Ok(signed) => signed,
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    // Never return a signed internal/private endpoint to the browser.
    let raw = serde_json::to_value(&signed).ok();
    let Some(url) = raw.as_ref().and_then(serde_json::Value::as_str) else {
        return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
    };
    let Ok(parsed) = url.parse::<Uri>() else {
        return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
    };
    let actual = parsed
        .authority()
        .zip(parsed.scheme_str())
        .map(|(authority, scheme)| format!("{scheme}://{authority}"));
    if actual.as_deref() != Some(state.s3_origin.as_str()) {
        return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
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
    let mut response = private_response(response, &[]);
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
