//! C-12/S065: live-administrator-only edition history of attached ciphertext.
//! No buyer or Layer route accepts a client-selected historical version.
use super::*;
use serde_json::json;
use tor_commerce::repositories::admin_reader_entitlements::
    PgAdministratorHistoricalEditionEntitlements;
use tor_contracts::paid_delivery::ObjectId;
use tor_reading::services::paid_delivery::issue_paid_admin_asset_grant;

/// Authenticate the *current* A session, role and double-submit browser CSRF.
/// D repeats the live entitlement before and after private G signing.
async fn authenticate_sealed_admin(
    state: &GrantState,
    headers: &HeaderMap,
) -> Result<(PositiveId, [u8; 32]), StatusCode> {
    let Some(session) = state.cookies.session_token(headers) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&session);
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.administrator => actor,
        Ok(Some(_)) => return Err(StatusCode::FORBIDDEN),
        Ok(None) => return Err(StatusCode::UNAUTHORIZED),
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|csrf| state.cookies.valid_csrf(headers, csrf))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let actor_id = PositiveId::new(actor.actor.user_id.get())
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok((actor_id, digest))
}

fn private_json<T: serde::Serialize>(content: T) -> Response {
    let mut response = private_response(Json(content), &[]);
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

pub(super) async fn list(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (actor_id, digest) = match authenticate_sealed_admin(&state, &headers).await {
        Ok(authorized) => authorized,
        Err(error) => return deny(error),
    };
    let Some(chapter_id) = positive(&raw_chapter) else {
        return deny(StatusCode::NOT_FOUND);
    };
    if !state.rate_limits.lock().await.allow(actor_id.get(), Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let d = PgAdministratorReaderEntitlements::new(
        state.accounts.pool().clone(),
        digest,
    );
    let editions = match d.sealed_history(Actor { user_id: actor_id }, chapter_id).await {
        Ok(editions) if !editions.is_empty() => editions,
        Ok(_) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(status(error)),
    };
    let versions: Vec<_> = editions.iter().map(|(id, no)| json!({
        "version_id": id.wire(),
        "version_no": no,
    })).collect();
    private_json(json!({
        "schema": "tor.admin_sealed_history.v1",
        "chapter_id": chapter_id.wire(),
        "versions": versions,
    }))
}

pub(super) async fn grant(
    State(state): State<GrantState>,
    Path((raw_chapter, raw_version)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (actor_id, digest) = match authenticate_sealed_admin(&state, &headers).await {
        Ok(authorized) => authorized,
        Err(error) => return deny(error),
    };
    let (Some(chapter_id), Some(version_id)) =
        (positive(&raw_chapter), positive(&raw_version))
    else {
        return deny(StatusCode::NOT_FOUND);
    };
    if !state.rate_limits.lock().await.allow(actor_id.get(), Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let Ok(_permit) = state.concurrent.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };
    let d = PgAdministratorHistoricalEditionEntitlements::new(
        state.accounts.pool().clone(),
        digest,
        version_id,
    );
    let result = issue_paid_chapter_grant(
        &d,
        &state.admin_versions,
        state.storage.as_ref(),
        Actor { user_id: actor_id },
        chapter_id,
        None,
    ).await;
    let granted = match result {
        Ok(granted) if granted.version_id == version_id.wire() => granted,
        Ok(_) => return deny(StatusCode::CONFLICT),
        Err(error) => return deny(status(error)),
    };
    if let Err(error) = state.admin_audit.record(
        &digest,
        actor_id,
        chapter_id,
        version_id,
        AdminReadAction::SealedChapterGrant,
    ).await {
        return deny(status(error));
    }
    private_json(granted)
}

pub(super) async fn asset(
    State(state): State<GrantState>,
    Path((raw_chapter, raw_version, raw_object)):
        Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let (actor_id, digest) = match authenticate_sealed_admin(&state, &headers).await {
        Ok(authorized) => authorized,
        Err(error) => return deny(error),
    };
    let (Some(chapter_id), Some(version_id), Ok(object_id)) = (
        positive(&raw_chapter),
        positive(&raw_version),
        ObjectId::parse(&raw_object),
    ) else {
        return deny(StatusCode::NOT_FOUND);
    };
    if !state.rate_limits.lock().await.allow(actor_id.get(), Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let Ok(_permit) = state.concurrent.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };
    let d = PgAdministratorHistoricalEditionEntitlements::new(
        state.accounts.pool().clone(),
        digest,
        version_id,
    );
    let result = issue_paid_admin_asset_grant(
        &d,
        &state.admin_assets,
        state.storage.as_ref(),
        Actor { user_id: actor_id },
        chapter_id,
        &object_id,
    ).await;
    let granted = match result {
        Ok(granted) if granted.version_id == version_id.wire() => granted,
        Ok(_) => return deny(StatusCode::CONFLICT),
        Err(error) => return deny(status(error)),
    };
    if let Err(error) = state.admin_audit.record(
        &digest,
        actor_id,
        chapter_id,
        version_id,
        AdminReadAction::SealedAssetGrant,
    ).await {
        return deny(status(error));
    }
    private_json(granted)
}
