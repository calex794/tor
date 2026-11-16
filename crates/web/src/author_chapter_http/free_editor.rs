//! S3-backed owner editing through the same forms and B optimistic revision.
use super::*;

pub(super) async fn page(
    state: &AuthorChapterState,
    actor: i64,
    book: i64,
    chapter: i64,
    headers: &HeaderMap,
) -> Result<Option<Response>, AuthorError> {
    let Some(publisher) = state.free_author.as_ref() else {
        return Ok(None);
    };
    let Some(owned) = publisher.owned(actor, book, chapter).await? else {
        return Ok(None);
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(headers);
    let key = generate_session_token();
    let html = if owned.published {
        crate::author_chapter_pages::edit_published_chapter(
            book,
            chapter,
            &owned.chapter,
            &csrf,
            &key,
        )
    } else {
        crate::author_chapter_pages::edit_chapter(book, chapter, &owned.chapter, &csrf, &key)
    };
    Ok(Some(private_response(html, &cookies)))
}

pub(super) async fn update(
    state: &AuthorChapterState,
    actor: i64,
    book: i64,
    chapter: i64,
    form: &EditChapterForm,
) -> Result<Option<Response>, AuthorError> {
    let Some(publisher) = state.free_author.as_ref() else {
        return Ok(None);
    };
    if publisher.owned(actor, book, chapter).await?.is_none() {
        return Ok(None);
    }
    let body = form.body.replace("\r\n", "\n").replace('\r', "\n");
    let input = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: Some(chapter as u64),
        number: form.number,
        expected_revision: form.expected_revision,
        title: &form.title,
        body: &body,
        access: "free",
        price_amount: 0,
        command_key: &form.command_key,
    })?;
    publisher.publish_edit(actor, &input).await?;
    Ok(Some(private_response(
        Redirect::to(&format!("/me/books/{book}")),
        &[],
    )))
}

pub(super) fn error(error: AuthorError) -> Response {
    deny(match error {
        AuthorError::Invalid => StatusCode::BAD_REQUEST,
        AuthorError::Missing => StatusCode::NOT_FOUND,
        AuthorError::Forbidden => StatusCode::FORBIDDEN,
        AuthorError::Conflict => StatusCode::CONFLICT,
        AuthorError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    })
}
