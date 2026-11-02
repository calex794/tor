//! HTTP registration of books. B owns canonical identity and persistence.
use super::*;

pub(super) async fn create_book(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<NewBookForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let identity = match source_identity(form.source_url.as_deref().unwrap_or(""), actor, &form.command_key) {
        Ok(identity) => identity,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let request = match new_book_with_taxonomy(
        &identity,
        &form.target_language,
        &form.title,
        &form.command_key,
        form.author_name.as_deref().unwrap_or(""),
        form.genres.as_deref().unwrap_or(""),
        TaxonomyFields {
            tags: form.tags.as_deref().unwrap_or(""),
            fandoms: form.fandoms.as_deref().unwrap_or(""),
        },
    )
    .and_then(|book| with_book_description(book, form.description.as_deref().unwrap_or("")))
    .and_then(|book| with_registration_metadata(book, RegistrationFields {
        source_url: form.source_url.as_deref().unwrap_or(""),
        original_title: form.original_title.as_deref().unwrap_or(""),
        original_language: form.original_language.as_deref().unwrap_or(""),
        release_year: form.release_year.as_deref().unwrap_or(""),
        project_status: form.project_status.as_deref().unwrap_or(""),
        catalog_section: form.catalog_section.as_deref().unwrap_or(""),
        alt_titles: [form.alt_title_1.as_deref().unwrap_or(""), form.alt_title_2.as_deref().unwrap_or(""), form.alt_title_3.as_deref().unwrap_or("")],
    }))
    {
        Ok(request) => request,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    match state.authoring.create_owned_book(actor, &request).await {
        Ok(id) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
