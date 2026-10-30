//! Passive /books/{id}?ref= capture only *after* public visibility succeeds.
#![forbid(unsafe_code)]
use crate::{
    auth_cookies::private_response,
    book_follow_page::FollowBookControl,
};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_referrals::{
    domain::valid_code,
    repositories::{postgres::CaptureResult, PgReferrals},
};

/// No URL decoding, redirects, or user-controlled target paths. Invalid or
/// duplicated ref parameters are ignored, preserving the ordinary book page.
fn query_referral(query: Option<&str>) -> Option<&str> {
    let query = query.filter(|query| query.len() <= 2048)?;
    let mut values = query.split('&').filter_map(|pair| pair.strip_prefix("ref="));
    let value = values.next()?;
    if values.next().is_some() || !valid_code(value) {
        return None;
    }
    Some(value)
}

pub(super) async fn capture(
    State(control): State<FollowBookControl>,
    request: Request,
    next: Next,
) -> Response {
    let code = query_referral(request.uri().query()).map(str::to_owned);
    let path = request.uri().path().to_owned();
    let headers: HeaderMap = request.headers().clone();

    // The repository must never see hidden, missing or non-canonical book
    // links: the existing book_detail ACL is the sole visibility authority.
    let response = next.run(request).await;
    if response.status() != StatusCode::OK {
        return response;
    }
    let Some(code) = code else {
        return response;
    };
    let (accounts, cookies) = control.browser_context();
    // Authenticated readers cannot claim a new signup invitation.
    if let Some(token) = cookies.session_token(&headers) {
        match accounts.active_session_user(&session_token_digest(&token)).await {
            Ok(Some(_)) | Err(_) => return private_response(response, &[]),
            Ok(None) => {}
        }
    }
    let existing = cookies.guest_token(&headers);
    let token = existing.clone().unwrap_or_else(generate_session_token);
    let digest = session_token_digest(&token);
    let repo = PgReferrals::new(accounts.pool().clone());
    match repo.capture(&digest, &code, "book_ref_query", &path).await {
        Ok(CaptureResult::Saved | CaptureResult::Existing) if existing.is_none() => {
            private_response(response, &[cookies.guest_cookie(&token)])
        }
        // A broken/stale link or PG outage must not erase an accessible book.
        _ => private_response(response, &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::query_referral;

    #[test]
    fn f_09_ref_query_is_bounded_and_unambiguous() {
        assert_eq!(query_referral(Some("ref=r_valid-123")), Some("r_valid-123"));
        assert_eq!(query_referral(Some("limit=1&ref=abc123&after=1")), Some("abc123"));
        for query in [
            "ref=bad",
            "ref=abc%2F123",
            "ref=%3Cscript%3E",
            "ref=abc123&ref=def456",
            "ref=abc123&ref=bad",
            "a=ref=abc123",
            "ref=",
            "ref=abcdef/../../",
        ] {
            assert_eq!(query_referral(Some(query)), None);
        }
        assert_eq!(query_referral(Some(&"x".repeat(2049))), None);
        assert_eq!(query_referral(None), None);
    }
}
