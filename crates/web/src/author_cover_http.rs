//! H-web-11: browser cover upload, only for the signed-in book owner.
//! An HTTP client never chooses an S3 key, owner ID or cover-slot revision.
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::post,
    Router,
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::repositories::{cover_slots::PgCoverSlots, postgres::PgAuthoring};
use tor_ingest::{
    domain::cover::CoverError,
    repositories::cover_jobs::PgCoverJobs,
    services::{
        cover_jobs::CoverObjectStore,
        image_pipeline::{validate_cover_source, MAX_SOURCE_BYTES, RECIPE_VERSION},
    },
};

use crate::auth_cookies::{private_response, CookiePolicy};

const MAX_MULTIPART_BYTES: usize = MAX_SOURCE_BYTES + 64 * 1024;

#[derive(Clone)]
struct CoverUploadState {
    accounts: PgAccounts,
    authoring: PgAuthoring,
    slots: PgCoverSlots,
    jobs: PgCoverJobs,
    storage: Arc<dyn CoverObjectStore>,
    cookies: CookiePolicy,
    pending: Arc<Semaphore>,
}

pub(crate) fn router(
    accounts: PgAccounts,
    cookies: CookiePolicy,
    storage: Arc<dyn CoverObjectStore>,
) -> Router {
    let pool = accounts.pool().clone();
    let state = CoverUploadState {
        accounts,
        authoring: PgAuthoring::new(pool.clone()),
        slots: PgCoverSlots::new(pool.clone()),
        jobs: PgCoverJobs::new(pool),
        storage,
        cookies,
        pending: Arc::new(Semaphore::new(2)),
    };
    Router::new()
        .route("/me/books/{id}/cover", post(upload_cover))
        .layer(DefaultBodyLimit::max(MAX_MULTIPART_BYTES))
        .with_state(state)
}

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
}

async fn upload_cover(
    State(state): State<CoverUploadState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if id <= 0 || !state.cookies.same_origin(&headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let actor = match state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
    {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // Enforce book ownership before buffering any untrusted image or S3 I/O.
    match state.authoring.owned_book(actor, id).await {
        Ok(Some(_)) => (),
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Ok(_permit) = state.pending.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };
    let mut csrf = None;
    let mut source = None;
    let mut fields = 0;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return deny(StatusCode::BAD_REQUEST),
        };
        fields += 1;
        if fields > 2 {
            return deny(StatusCode::BAD_REQUEST);
        }
        match field.name() {
            Some("csrf_token") if csrf.is_none() => {
                let value = match field.text().await {
                    Ok(value) if value.len() <= 128 => value,
                    _ => return deny(StatusCode::BAD_REQUEST),
                };
                if !state.cookies.valid_csrf(&headers, &value) {
                    return deny(StatusCode::FORBIDDEN);
                }
                csrf = Some(value);
            }
            Some("cover") if source.is_none() && csrf.is_some() => {
                let mime = match field.content_type() {
                    Some("image/jpeg") => "image/jpeg",
                    Some("image/png") => "image/png",
                    Some("image/webp") => "image/webp",
                    _ => return deny(StatusCode::UNSUPPORTED_MEDIA_TYPE),
                };
                let bytes = match field.bytes().await {
                    Ok(value) if !value.is_empty() && value.len() <= MAX_SOURCE_BYTES => value,
                    _ => return deny(StatusCode::PAYLOAD_TOO_LARGE),
                };
                source = Some((mime, bytes));
            }
            _ => return deny(StatusCode::BAD_REQUEST),
        }
    }
    if !state
        .cookies
        .valid_csrf(&headers, csrf.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some((mime, bytes)) = source else {
        return deny(StatusCode::BAD_REQUEST);
    };
    // The original NEwBOO validator rejects disguised, animated, oversized
    // and metadata-bearing images before the trusted S3 PUT.
    let check_bytes = bytes.clone();
    if !matches!(
        tokio::task::spawn_blocking(move || validate_cover_source(&check_bytes, mime)).await,
        Ok(Ok(()))
    ) {
        return deny(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let checksum: [u8; 32] = Sha256::digest(&bytes).into();
    let key = format!(
        "staging/covers/sources/{id}/{}",
        generate_session_token()
    );
    match tokio::time::timeout(
        Duration::from_secs(20),
        state.storage.put_source_verified(&key, &bytes, mime, &checksum),
    )
    .await
    {
        Ok(Ok(())) => (),
        Ok(Err(CoverError::Invalid)) => return deny(StatusCode::BAD_REQUEST),
        Ok(Err(CoverError::Forbidden)) => return deny(StatusCode::FORBIDDEN),
        _ => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let expected_revision = match state.slots.current_slot(id).await {
        Ok(Some((_, revision))) => revision,
        Ok(None) => 0,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let source_id = match state
        .jobs
        .register_verified_source(id, actor, &key, mime, bytes.len() as i64, &checksum)
        .await
    {
        Ok(source_id) => source_id,
        Err(CoverError::Forbidden) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    match state
        .jobs
        .enqueue(source_id, id, actor, expected_revision, RECIPE_VERSION)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{id}/settings/covers")), &[]),
        Err(CoverError::Conflict) => deny(StatusCode::CONFLICT),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_are_bounded_and_use_only_server_generated_staging_keys() {
        assert_eq!(MAX_SOURCE_BYTES, 8 * 1024 * 1024);
        assert!(MAX_MULTIPART_BYTES > MAX_SOURCE_BYTES);
        let opaque = generate_session_token();
        assert_eq!(opaque.len(), 43);
        assert!(format!("staging/covers/sources/42/{opaque}")
            .starts_with("staging/covers/sources/"));
    }
}
