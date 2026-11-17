//! D-05: historical Layer FREE PACK reader, exact active frame only.
//! All object keys/ranges stay server-side, and D is repeated after remote IO.
use super::{deny, private_response, session_token_digest, PackHttpState};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
};
use sha2::{Digest, Sha256};
use tor_contracts::paid_delivery::PositiveId;
use tor_storage::{
    domain::provider::StorageError,
    services::{
        free_chapter::FreeChapterObject,
        free_pack_read::read_reauthorized_pack,
        free_placement::{
            plan_after_acl, FreePhysicalPlacement, FreeReadPlan, PackMetadata,
            ValidatedPackReference,
        },
    },
};

fn csrf_valid(cookies: &super::CookiePolicy, headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        == Some(cookies.origin.as_str())
        && cookies.same_origin(headers)
        && headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| cookies.valid_csrf(headers, value))
}

async fn actor(
    state: &PackHttpState,
    headers: &HeaderMap,
) -> Result<PositiveId, StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let id = state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    PositiveId::new(id).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

/// No browser-supplied version, placement, bucket, PACK offset or key.
async fn authorized_pack_plan(
    actor: PositiveId,
    state: &PackHttpState,
    chapter: PositiveId,
) -> Result<Option<FreeReadPlan>, StorageError> {
    let Some(version) = state
        .layer
        .archived_free_version(actor, chapter)
        .await
        .map_err(|_| StorageError::Unavailable)?
    else {
        return Ok(None);
    };
    let Some(source) = state
        .reference
        .archived_free_active_pack_metadata_only(chapter.get(), version.get())
        .await
        .map_err(|_| StorageError::Unavailable)?
    else {
        return Ok(None);
    };
    if source.bucket != state.bucket
        || !(1..=200_000).contains(&source.size_bytes)
        || source.chapter_id != chapter.get()
        || source.version_id != version.get()
    {
        return Err(StorageError::Unavailable);
    }
    let Some(candidate) = state
        .placement
        .active_verified_free_range(
            source.chapter_id,
            source.version_id,
            source.sha256,
            source.size_bytes,
            &source.bucket,
        )
        .await
        .map_err(|_| StorageError::Unavailable)?
    else {
        return Err(StorageError::Unavailable);
    };
    let edition = FreeChapterObject::from_metadata(
        source.chapter_id,
        source.version_id,
        source.sha256,
        source.size_bytes,
    )?;
    let metadata = PackMetadata {
        bucket: candidate.bucket,
        key: candidate.key,
        pack_bytes: candidate.pack_bytes,
        seek_table_start: candidate.seek_table_start,
        frame_offset: candidate.frame_offset,
        frame_bytes: candidate.frame_bytes,
        pack_sha256: candidate.pack_sha256,
        manifest_sha256: candidate.manifest_sha256,
    };
    let frame = ValidatedPackReference::new(edition.clone(), metadata)?;
    Ok(Some(plan_after_acl(
        &edition,
        FreePhysicalPlacement::Pack(frame),
    )?))
}

/// POST: bearer-free, same-origin, double-submit CSRF and one B-verified PACK
/// frame. G checks Content-Range and Zstd bounds, then C/D/B recheck authority
/// and active generation before the response exposes any bytes.
pub(super) async fn read_pack(
    State(state): State<PackHttpState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if headers.contains_key(header::RANGE) || headers.contains_key("if-range") {
        return deny(StatusCode::BAD_REQUEST);
    }
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if !csrf_valid(&state.cookies, &headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Ok(chapter) = PositiveId::new(chapter_id) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let _slot = match state.inflight.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return deny(StatusCode::TOO_MANY_REQUESTS),
    };
    let bytes = match read_reauthorized_pack(
        || authorized_pack_plan(actor, &state, chapter),
        &state.frame_store,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(StorageError::Invalid) => return deny(StatusCode::NOT_FOUND),
        Err(StorageError::Conflict) => return deny(StatusCode::CONFLICT),
        Err(StorageError::Corrupt | StorageError::Unavailable) => {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    // Third D/B check prevents publication or revocation after the read's
    // second check from authorizing a stale response.
    let final_plan = match authorized_pack_plan(actor, &state, chapter).await {
        Ok(Some(plan)) => plan,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Some(frame) = final_plan.pack_frame() else {
        return deny(StatusCode::CONFLICT);
    };
    let edition = frame.edition();
    let actual: [u8; 32] = Sha256::digest(&bytes).into();
    if bytes.len() as u64 != edition.size_bytes() || actual != *edition.sha256() {
        return deny(StatusCode::CONFLICT);
    }
    let mut response = private_response(
        ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], bytes),
        &[],
    );
    for (name, value) in [
        ("x-tor-free-version-id", edition.version_id().to_string()),
        ("x-tor-free-sha256", hex::encode(edition.sha256())),
        ("x-tor-free-size", edition.size_bytes().to_string()),
    ] {
        let Ok(value) = HeaderValue::from_str(&value) else {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        };
        response.headers_mut().insert(name, value);
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
    fn pack_grant_requires_same_origin_and_csrf() {
        let cookies = super::super::CookiePolicy {
            secure: false,
            origin: "https://tor.example.test".to_string(),
        };
        let mut headers = HeaderMap::new();
        let csrf = "A".repeat(43);
        headers.insert(header::ORIGIN, cookies.origin.parse().unwrap());
        headers.insert(header::COOKIE, format!("tor-dev-csrf={csrf}").parse().unwrap());
        headers.insert("x-csrf-token", csrf.parse().unwrap());
        assert!(csrf_valid(&cookies, &headers));
        headers.insert(header::ORIGIN, "https://evil.example.test".parse().unwrap());
        assert!(!csrf_valid(&cookies, &headers));
        headers.remove(header::ORIGIN);
        assert!(!csrf_valid(&cookies, &headers));
    }
}
