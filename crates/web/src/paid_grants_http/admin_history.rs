//! C-12/S065: bounded, private unsealed history for a live site administrator.
//! An historical edition is never an ordinary reader, buyer or Layer grant.
use super::*;
use axum::http::header;
use serde_json::json;

async fn authorize(
    state: &AdminReaderState,
    headers: &HeaderMap,
) -> Result<(PgAdministratorReaderEntitlements, PositiveId, [u8; 32]), StatusCode> {
    let session = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&session);
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.administrator => actor,
        Ok(Some(_)) => return Err(StatusCode::FORBIDDEN),
        Ok(None) => return Err(StatusCode::UNAUTHORIZED),
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let actor_id =
        PositiveId::new(actor.actor.user_id.get()).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
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
    Ok((
        PgAdministratorReaderEntitlements::new(state.accounts.pool().clone(), digest),
        actor_id,
        digest,
    ))
}

pub(super) async fn list(
    State(state): State<AdminReaderState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (authority, actor, _) = match authorize(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    let chapter = match positive(&raw_chapter) {
        Some(id) => id,
        None => return deny(StatusCode::NOT_FOUND),
    };
    if !state.history_rate_limits.lock().await.allow(actor.get(), Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let history = match authority.unsealed_history(Actor { user_id: actor }, chapter).await {
        Ok(history) => history,
        Err(error) => return deny(status(error)),
    };
    // No metadata for a nonexistent/unauthorized chapter. History is read only,
    // so the auditor records successful deliveries of text, not listings.
    if history.is_empty() {
        return deny(StatusCode::NOT_FOUND);
    }
    let entries: Vec<_> = history
        .iter()
        .map(|(id, number)| json!({"version_id": id.get().to_string(), "version_no": number}))
        .collect();
    private_json(json!({
        "schema": "tor.admin_history.v1",
        "chapter_id": chapter.get().to_string(),
        "versions": entries,
    }))
}

pub(super) async fn read(
    State(state): State<AdminReaderState>,
    Path((raw_chapter, raw_version)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (authority, actor, digest) = match authorize(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return deny(error),
    };
    let (Some(chapter), Some(version)) = (positive(&raw_chapter), positive(&raw_version)) else {
        return deny(StatusCode::NOT_FOUND);
    };
    if !state.history_rate_limits.lock().await.allow(actor.get(), Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let current = match authority.unsealed_historical_version(
        Actor { user_id: actor }, chapter, version,
    ).await {
        Ok(Some(pinned)) => pinned,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(status(error)),
    };
    let body = match state.admin_unsealed.read(current).await {
        Ok(Some(body)) => body,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(status(error)),
    };
    // A second authoritative DB check blocks revocation and a changed
    // chapter-version association while B was fetching the original text.
    match authority
        .unsealed_historical_version(Actor { user_id: actor }, chapter, version)
        .await
    {
        Ok(Some(pinned)) if pinned == current => {}
        Ok(_) => return deny(StatusCode::FORBIDDEN),
        Err(error) => return deny(status(error)),
    }
    if let Err(error) = state
        .admin_audit
        .record(
            &digest,
            actor,
            chapter,
            version,
            AdminReadAction::UnsealedBody,
        )
        .await
    {
        return deny(status(error));
    }
    private_json(json!({
        "schema": "tor.admin_unsealed.v1",
        "chapter_id": chapter.get().to_string(),
        "version_id": version.get().to_string(),
        "body_markdown": body,
    }))
}

fn private_json(body: serde_json::Value) -> Response {
    let mut response = private_response(Json(body), &[]);
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
        .headers_mut()
        .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}
