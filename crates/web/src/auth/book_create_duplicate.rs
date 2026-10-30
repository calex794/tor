//! B-19 browser-side title review; the B domain owns matching and SQL.
use super::*;

pub(super) async fn check_book_title(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<NewBookForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(
            &headers,
            form.csrf_token.as_deref().unwrap_or(""),
        )
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    if !tor_books::services::authoring::valid_command_key(&form.command_key) {
        return deny(StatusCode::BAD_REQUEST);
    }
    // Validate all create fields, not just the name; keep the review page
    // consistent with what can actually be submitted.
    let identity = format!("manual:{actor}:{}", form.command_key);
    let valid = new_book_with_taxonomy(
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
    .and_then(|book| tor_books::services::title_duplicates::with_title_aliases(
        book,
        form.original_title.as_deref().unwrap_or(""),
        form.alternative_titles.as_deref().unwrap_or(""),
    ));
    if valid.is_err() {
        return deny(StatusCode::BAD_REQUEST);
    }
    let names = match tor_books::services::title_duplicates::candidate_titles(
        &form.title,
        form.original_title.as_deref().unwrap_or(""),
        form.alternative_titles.as_deref().unwrap_or(""),
    ) {
        Ok(names) => names,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let candidates = match state.authoring.title_candidates(actor, &names).await {
        Ok(matches) => matches,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    private_response(author_pages::review_new_book(&form, &candidates), &[])
}
