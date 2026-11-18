//! FND-00: current paid S3-only owner editor and POST metadata selection.
//! Neither the browser nor the legacy database editor chooses a S3 key.
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
    let command_key = generate_session_token();
    Ok(Some(private_response(
        crate::author_chapter_pages::edit_published_paid_chapter(
            book,
            chapter,
            &owned,
            &csrf,
            &command_key,
        ),
        &cookies,
    )))
}

pub(super) async fn price(
    state: &AuthorChapterState,
    actor: i64,
    book: i64,
    chapter: i64,
) -> Result<Option<i64>, AuthorError> {
    let storage = PgPaidStorage::new(state.accounts.pool().clone());
    if let Some(metadata) = storage
        .owned_published_bodyless_paid_metadata(actor, book, chapter)
        .await?
    {
        return Ok(Some(metadata.price_amount));
    }
    // Only already-retained PG-body paid editions use this compatibility query.
    // Newly published versions never fall back to the legacy text reader.
    Ok(storage
        .owned_published_paid_chapter(actor, book, chapter)
        .await?
        .map(|legacy| legacy.price_amount))
}
