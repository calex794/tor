//! DROP-06a: independently mountable, private server-side Range gateway.
//! Never mounted by main.rs or merged into the live S349 reader. This module
//! exposes the exact HTTP/browser contract for disposable acceptance only.
//! Runtime cutover requires its own gated DROP-06b change.
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    free_pack_gateway::{read_authorized_active, FreePackReadMode},
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::repositories::free_pack_attach::PgFreePackCatalog;
use tor_reading::repositories::free_s3_ref::PgFreeS3Ref;
use tor_storage::{
    domain::provider::{valid_private_bucket, StorageError},
    repositories::free_pack_range_s3::S3PackFrameStore,
};

#[derive(Clone)]
struct PackHttpState {
    reference: PgFreeS3Ref,
    placement: PgFreePackCatalog,
    frame_store: S3PackFrameStore,
    cookies: CookiePolicy,
    bucket: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Lookup {
    mode: Option<String>,
    version_id: Option<i64>,
}

/// Explicitly separate router. main.rs MUST NOT mount it until the active
/// PACK browser contract, saved V1 and operational feature-gate acceptance.
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
    let pool = accounts.pool().clone();
    Ok(Router::new()
        .route("/api/v1/chapters/{id}/free-pack-source", get(source))
        .with_state(PackHttpState {
            reference: PgFreeS3Ref::new(pool.clone()),
            placement: PgFreePackCatalog::new(pool),
            frame_store: S3PackFrameStore::new(s3),
            cookies: CookiePolicy { secure, origin },
            bucket,
        }))
}

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
}

async fn source(
    State(state): State<PackHttpState>,
    Path(chapter): Path<i64>,
    Query(query): Query<Lookup>,
    headers: HeaderMap,
) -> Response {
    if chapter <= 0
        || headers.contains_key(header::RANGE)
        || headers.contains_key("if-range")
    {
        return deny(StatusCode::BAD_REQUEST);
    }
    // No cross-origin content grants; the session cookie is SameSite=Strict.
    // Anonymous public/basic access still returns no CORS permission.
    if headers.get("sec-fetch-site").and_then(|s| s.to_str().ok()) == Some("cross-site")
        || headers.get(header::ORIGIN).is_some_and(|s| {
            s.to_str().ok() != Some(state.cookies.origin.as_str())
        })
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let mode = match (query.mode.as_deref(), query.version_id) {
        (None, None) => FreePackReadMode::Published,
        (Some("saved"), Some(version)) if version > 0 => {
            FreePackReadMode::Saved { version_id: version }
        }
        _ => return deny(StatusCode::BAD_REQUEST),
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
