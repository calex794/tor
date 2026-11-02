//! Paid reader text corrections: entitlement-first, exact-version and
//! server-checked quote. No decrypted S3 content is returned to any caller.
use super::{deny, positive, GrantState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use tor_accounts::services::security::session_token_digest;
use tor_contracts::paid_delivery::{Actor, EntitlementDecision, EntitlementPort, PositiveId};
use tor_discussions::repositories::text_issues::{
    CreatedIssue, IssueError, NewTextIssue, PgTextIssues,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaidIssueBody {
    purchase_id: String,
    version_id: String,
    block_index: i32,
    selection_start: i32,
    selection_end: i32,
    quote: String,
    suggested_text: String,
    issue_type: String,
}

#[derive(Serialize)]
struct Receipt {
    status: &'static str,
}

fn issue_error(error: IssueError) -> StatusCode {
    match error {
        IssueError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        IssueError::NotVisible => StatusCode::FORBIDDEN,
        IssueError::Conflict => StatusCode::CONFLICT,
        IssueError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(super) async fn submit(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PaidIssueBody>,
) -> Response {
    if state.reader_s3_origin.is_none() {
        return deny(StatusCode::NOT_FOUND);
    }
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
    if headers.get(header::ORIGIN).and_then(|value| value.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Some(chapter_id), Some(purchase_id), Some(version_id), Ok(actor_id)) = (
        positive(&raw_chapter),
        positive(&body.purchase_id),
        positive(&body.version_id),
        PositiveId::new(actor),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };

    // No reporter-selected edition or purchase is authoritative. D decides
    // first, and the PostgreSQL insertion repeats D within its transaction.
    let access = state
        .entitlement
        .resolve_entitlement(Actor { user_id: actor_id }, chapter_id, Some(purchase_id))
        .await;
    match access {
        Ok(EntitlementDecision::Allowed { pinned })
            if pinned.chapter_id == chapter_id && pinned.version_id == version_id => {}
        Ok(_) => return deny(StatusCode::FORBIDDEN),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    // Cross-instance rate limiting, separate from signed-grant limits.
    let key = session_token_digest(&format!("action:paid_text_issue:user:{actor}"));
    match state.accounts.permit_auth_attempt(&key, 20, 60).await {
        Ok(true) => {}
        Ok(false) => return deny(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }

    let repo = PgTextIssues::new(state.accounts.pool().clone());
    let result = repo
        .create_paid(
            actor,
            purchase_id.get(),
            NewTextIssue {
                chapter_id: chapter_id.get(),
                version_id: version_id.get(),
                block_index: body.block_index,
                selection_start: body.selection_start,
                selection_end: body.selection_end,
                quote: &body.quote,
                suggestion: &body.suggested_text,
                issue_type: &body.issue_type,
            },
        )
        .await;
    let status = match result {
        Ok(CreatedIssue::Added(_)) => "sent",
        Ok(CreatedIssue::Replayed(_)) => "repeat",
        Err(error) => return deny(issue_error(error)),
    };
    let mut response = private_response(Json(Receipt { status }), &[]);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::PaidIssueBody;

    #[test]
    fn json_requires_exact_purchase_version_and_no_client_authority_override() {
        let valid = r#"{"purchase_id":"31","version_id":"42","block_index":1,
            "selection_start":0,"selection_end":7,"quote":"PRIVATE",
            "suggested_text":"Correct","issue_type":"typo"}"#;
        assert!(serde_json::from_str::<PaidIssueBody>(valid).is_ok());
        for unsafe_body in [
            r#"{"purchase_id":"31","version_id":"42","block_index":1,
                "selection_start":0,"selection_end":7,"quote":"PRIVATE",
                "suggested_text":"Correct","issue_type":"typo","user_id":"99"}"#,
            r#"{"version_id":"42","block_index":1,"selection_start":0,
                "selection_end":7,"quote":"PRIVATE","suggested_text":"Correct","issue_type":"typo"}"#,
        ] {
            assert!(serde_json::from_str::<PaidIssueBody>(unsafe_body).is_err());
        }
    }
}
