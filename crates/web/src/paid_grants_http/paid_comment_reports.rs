//! S049: buyer-scoped complaint about a visible exact-edition paid comment.
//! Reporting never returns the comment body, selected quote or any S3 material.
use super::{deny, paid_comments, positive, GrantState};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use tor_social::repositories::{
    book_shares::BookShareError, paid_comment_reports::PgPaidCommentReports,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReportInput {
    purchase_id: String,
    version_id: String,
    comment_id: String,
    reason: String,
    details: String,
    command_key: String,
}

pub(super) async fn submit(
    State(state): State<GrantState>,
    Path(chapter_raw): Path<String>,
    headers: HeaderMap,
    Json(input): Json<ReportInput>,
) -> Response {
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
    let (Some(chapter), Some(purchase), Some(version), Some(comment)) = (
        positive(&chapter_raw),
        positive(&input.purchase_id),
        positive(&input.version_id),
        positive(&input.comment_id),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let (actor, digest) =
        match paid_comments::authorized(&state, &headers, chapter, purchase, version).await {
            Ok(auth) => auth,
            Err(status) => return deny(status),
        };
    let reports = PgPaidCommentReports::new(state.accounts.pool().clone());
    match reports
        .submit(
            &digest,
            actor,
            (purchase.get(), chapter.get(), version.get(), comment.get()),
            &input.reason,
            &input.details,
            &input.command_key,
        )
        .await
    {
        Ok(id) => paid_comments::private_json(json!({"status":"sent","id":id})),
        Err(BookShareError::Invalid) => deny(StatusCode::UNPROCESSABLE_ENTITY),
        Err(BookShareError::NotFound) => deny(StatusCode::NOT_FOUND),
        Err(BookShareError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(BookShareError::RateLimited) => deny(StatusCode::TOO_MANY_REQUESTS),
        Err(BookShareError::Conflict) => deny(StatusCode::CONFLICT),
        Err(BookShareError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_reports_reject_unknown_authority_and_missing_target() {
        let value = r#"{"purchase_id":"5","version_id":"7","comment_id":"9",
          "reason":"spam","details":"junk","command_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#;
        assert!(serde_json::from_str::<ReportInput>(value).is_ok());
        assert!(serde_json::from_str::<ReportInput>(
            &value.replace("\"reason\":\"spam\"", "\"user_id\":11,\"reason\":\"spam\"")
        ).is_err());
        assert!(serde_json::from_str::<ReportInput>(
            &value.replace("\"comment_id\":\"9\"", "\"comment_id\":null")
        ).is_err());
    }
}
