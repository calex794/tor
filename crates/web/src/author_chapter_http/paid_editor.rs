//! FND-00g: isolated S3-only paid editor integration at the authenticated
//! author HTTP boundary. A missing bodyless source never falls back to PG.
use super::*;
use tor_books::repositories::paid_storage::PgPaidStorage;

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

/// Legacy paid V1 path remains separate until the private S3-only editor and
/// version-bound migration have both completed their integration acceptance.
pub(super) async fn legacy_update(
    state: &AuthorChapterState,
    actor: i64,
    id: i64,
    chapter_id: i64,
    book: u64,
    chapter: u64,
    form: &EditChapterForm,
) -> Option<Response> {
    // B decides paid/publication state, never a submitted "paid" form field.
    // The owner query and the publishing transaction both check ownership.
    let paid = if state.paid_author.is_some() {
        match PgPaidStorage::new(state.accounts.pool().clone())
            .owned_published_paid_chapter(actor, id, chapter_id)
            .await
        {
            Ok(paid) => paid,
            Err(_) => return Some(deny(StatusCode::SERVICE_UNAVAILABLE)),
        }
    } else {
        None
    };
    if let (Some(paid), Some(publisher)) = (paid, state.paid_author.as_ref()) {
        let body = form.body.replace("\r\n", "\n").replace('\r', "\n");
        let input = match save_chapter(SaveChapterRequest {
            book_id: book,
            chapter_id: Some(chapter),
            number: form.number,
            expected_revision: form.expected_revision,
            title: &form.title,
            body: &body,
            access: "paid",
            price_amount: paid.price_amount,
            command_key: &form.command_key,
        }) {
            Ok(input) => input,
            Err(_) => return Some(deny(StatusCode::BAD_REQUEST)),
        };
        return Some(match publisher.publish_edit(actor, &input).await {
            Ok(()) => private_response(Redirect::to(&format!("/me/books/{id}/issues")), &[]),
            Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
            Err(AuthorError::Conflict) => private_response(
                (
                    StatusCode::CONFLICT,
                    "Глава уже изменена. Обновите страницу редактора.",
                ),
                &[],
            ),
            Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
            Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
            Err(AuthorError::Unavailable) => private_response(
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Защищённое сохранение не завершено. Повторите эту же форму.",
                ),
                &[],
            ),
        });
    }
    None
}
