//! Author-only scheduled free-chapter commands (S058).
use super::*;

#[derive(Deserialize)]
pub(super) struct ScheduleChapterForm {
    expected_revision: i64,
    version_id: i64,
    scheduled_utc: String,
    csrf_token: Option<String>,
}

pub(super) async fn schedule_chapter(
    State(state): State<AuthorChapterState>,
    Path((id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<ScheduleChapterForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state
        .authoring
        .schedule_pending_free_chapter(
            actor,
            id,
            chapter_id,
            form.expected_revision,
            form.version_id,
            &form.scheduled_utc,
        )
        .await
    {
        Ok(()) => private_response(
            Redirect::to(&format!("/me/books/{id}/chapters/{chapter_id}/release")),
            &[],
        ),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn cancel_schedule(
    State(state): State<AuthorChapterState>,
    Path((id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<PublishForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state
        .authoring
        .cancel_scheduled_free_chapter(actor, id, chapter_id)
        .await
    {
        Ok(()) => private_response(
            Redirect::to(&format!("/me/books/{id}/chapters/{chapter_id}/release")),
            &[],
        ),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
