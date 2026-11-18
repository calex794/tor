//! FND-00g: isolated S3-only paid editor integration at the authenticated
//! author HTTP boundary. A missing bodyless source never falls back to PG.
use super::*;

pub(super) async fn page(
    state: &AuthorChapterState,
    actor: i64,
    book: i64,
    chapter: i64,
    headers: &HeaderMap,
) -> Result<Option<Response>, AuthorError> {
    let Some(publisher) = state.paid_author.as_ref() else {
        return Ok(None);
    };
    let Some(owned) = publisher.owned_bodyless(actor, book, chapter).await? else {
        return Ok(None);
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(headers);
    let key = generate_session_token();
    let html = crate::author_chapter_pages::edit_published_paid_chapter(
        book, chapter, &owned, &csrf, &key,
    );
    Ok(Some(private_response(html, &cookies)))
}

pub(super) async fn update(
    state: &AuthorChapterState,
    actor: i64,
    book: i64,
    chapter: i64,
    form: &EditChapterForm,
) -> Result<Option<Response>, AuthorError> {
    let Some(publisher) = state.paid_author.as_ref() else {
        return Ok(None);
    };
    // Metadata is B-owned. Never trust a submitted access level, price, title
    // or chapter number, and never fetch plaintext to authorize a POST.
    let Some(meta) = PgPaidStorage::new(state.accounts.pool().clone())
        .owned_published_bodyless_paid_metadata(actor, book, chapter)
        .await?
    else {
        return Ok(None);
    };
    let book_id = u64::try_from(book).map_err(|_| AuthorError::Invalid)?;
    let chapter_id = u64::try_from(chapter).map_err(|_| AuthorError::Invalid)?;
    let body = form.body.replace("\r\n", "\n").replace('\r', "\n");
    let input = save_chapter(SaveChapterRequest {
        book_id,
        chapter_id: Some(chapter_id),
        number: meta.number,
        expected_revision: form.expected_revision,
        title: &meta.title,
        body: &body,
        access: "paid",
        price_amount: meta.price_amount,
        command_key: &form.command_key,
    })?;
    // This port is deliberately unavailable by default. The concrete S3
    // publisher will be enabled only with the verified owner-source reader.
    publisher.publish_edit_bodyless(actor, &input).await?;
    Ok(Some(private_response(
        Redirect::to(&format!("/me/books/{book}/issues")),
        &[],
    )))
}
