//! Separate operator action for a currently approved public cover.
//! No S3 delete occurs on the HTTP path: B revokes projections atomically,
//! and the existing durable publication worker retires stale CDN copies.
use super::*;
use axum::response::Response;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WithdrawForm {
    csrf_token: String,
    expected_slot_revision: i64,
    reason: String,
    confirmed: Option<String>,
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<WithdrawForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(digest) = session(&state, &headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let reason = form.reason.trim();
    let back = format!("/moderation/books/{book_id}/cover-history");
    if book_id <= 0 || form.expected_slot_revision <= 0
        || reason.is_empty() || reason.len() > 2000
    {
        return super::super::moderation_books::decision_error(StatusCode::BAD_REQUEST, &back);
    }
    let confirmed = match super::super::moderation_confirm::was_confirmed(form.confirmed.as_deref()) {
        Ok(value) => value,
        Err(code) => return super::super::moderation_books::decision_error(code, &back),
    };
    if !confirmed {
        return super::super::moderation_confirm::confirmation_page(
            super::super::moderation_confirm::ReviewIntent {
                kind: "cover_withdraw",
                subject_id: book_id,
                revision: form.expected_slot_revision,
                action: "withdraw",
                reason,
                csrf: &form.csrf_token,
            },
        );
    }
    let repository = PgCoverReview::new(state.accounts.pool().clone());
    match repository
        .withdraw_current(&digest, book_id, form.expected_slot_revision, reason)
        .await
    {
        Ok(()) => private_response(Redirect::to(&back), &[]),
        Err(error) => super::super::moderation_books::decision_error(error_code(error), &back),
    }
}
