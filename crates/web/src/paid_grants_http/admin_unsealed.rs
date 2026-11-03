//! S065 / C-12: private unsealed chapter delivery for a live site administrator.
//! No plaintext in SSR; only same-origin POST, session + role + CSRF can read
//! the current B working draft or published edition. No purchase/progress write.
use super::*;
use serde_json::json;

pub(super) async fn read(
    State(state): State<AdminReaderState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&session);
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.administrator => actor,
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(chapter_id) = positive(&raw_chapter) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let Ok(actor_id) = PositiveId::new(actor.actor.user_id.get()) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !state
        .rate_limits
        .lock()
        .await
        .allow(actor.actor.user_id.get(), Instant::now())
    {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let authority = PgAdministratorReaderEntitlements::new(state.accounts.pool().clone(), digest);
    let admin = Actor { user_id: actor_id };
    let pinned = match authority.unsealed_version(admin, chapter_id).await {
        Ok(Some(version)) => version,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(status(error)),
    };
    let body = match state.admin_unsealed.read(pinned).await {
        Ok(Some(body)) => body,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(status(error)),
    };
    // Re-check the real live session/role and the current version after B
    // returns. A revoked administrator or a concurrent draft switch cannot
    // receive a stale privileged response.
    match authority.unsealed_version(admin, chapter_id).await {
        Ok(Some(current)) if current == pinned => {}
        Ok(Some(_)) => return deny(StatusCode::CONFLICT),
        Ok(None) => return deny(StatusCode::FORBIDDEN),
        Err(error) => return deny(status(error)),
    }
    // Never return privileged plaintext unless the live A-role access is
    // committed to an immutable audit. The audit does not persist text.
    if let Err(error) = state
        .admin_audit
        .record(
            &digest,
            actor_id,
            chapter_id,
            pinned.version_id,
            AdminReadAction::UnsealedBody,
        )
        .await
    {
        return deny(status(error));
    }
    let mut response = private_response(
        Json(json!({
            "schema": "tor.admin_unsealed.v1",
            "chapter_id": chapter_id.get().to_string(),
            "version_id": pinned.version_id.get().to_string(),
            "body_markdown": body,
        })),
        &[],
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
