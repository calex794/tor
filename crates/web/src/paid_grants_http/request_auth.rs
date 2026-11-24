//! REUSE-02A: request authentication for the two paid delivery grant POSTs.
use super::*;

/// Preserve session-first denial precedence and strict JSON Origin + CSRF.
/// This is not a chapter/asset entitlement or administrator role check.
pub(super) async fn authenticate_grant(
    state: &GrantState,
    headers: &HeaderMap,
) -> Result<(String, i64), StatusCode> {
    let session = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let actor = match state
        .accounts
        .active_session_user(&session_token_digest(&session))
        .await
    {
        Ok(Some(actor)) => actor,
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
    Ok((session, actor))
}
