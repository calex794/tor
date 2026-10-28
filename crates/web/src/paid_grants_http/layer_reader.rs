//! D-05/H: GET shell for an authorized Layer CURRENT chapter.
//! The POST grant independently re-checks the same Layer entitlement.
use super::*;

pub(super) async fn layer_reader(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(s3_origin) = state.reader_s3_origin.as_deref() else {
        return deny(StatusCode::NOT_FOUND);
    };
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let actor = match state
        .accounts
        .active_session_user(&session_token_digest(&session))
        .await
    {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (Some(chapter_id), Ok(actor_id)) = (positive(&raw_chapter), PositiveId::new(actor))
    else {
        return deny(StatusCode::NOT_FOUND);
    };

    match state
        .layer
        .resolve_entitlement(Actor { user_id: actor_id }, chapter_id, None)
        .await
    {
        Ok(EntitlementDecision::Allowed { pinned }) if pinned.chapter_id == chapter_id => {}
        Ok(EntitlementDecision::Allowed { .. }) => return deny(StatusCode::CONFLICT),
        Ok(EntitlementDecision::Denied { reason }) => {
            return deny(match reason {
                tor_contracts::paid_delivery::DenialReason::Hidden => StatusCode::NOT_FOUND,
                tor_contracts::paid_delivery::DenialReason::Unknown => {
                    StatusCode::SERVICE_UNAVAILABLE
                }
                _ => StatusCode::FORBIDDEN,
            });
        }
        Err(error) => return deny(status(error)),
    }

    // No chapter text, S3 object path, DEK or signed GET in the HTML document.
    // The browser POST has independent session, Origin, CSRF and rate limits.
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        pages::layer_reader_shell(chapter_id.get(), &csrf, s3_origin),
        &cookies,
    );
    let csp = format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
         connect-src 'self' {s3_origin}; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    );
    let Ok(value) = HeaderValue::from_str(&csp) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    response.headers_mut().insert("content-security-policy", value);
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
        .headers_mut()
        .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}
