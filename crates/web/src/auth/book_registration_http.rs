//! Build the S019 author request; PostgreSQL writes stay in the audited HTTP entry.
use super::*;
use tor_books::services::{
    authoring::{new_book_with_taxonomy, with_book_description, TaxonomyFields},
    book_registration::{source_identity, with_extra_source_links, with_registration_metadata, RegistrationFields},
};

pub(super) fn request(
    actor: i64,
    form: &NewBookForm,
) -> Result<tor_books::services::authoring::NewBook, AuthorError> {
    let identity = source_identity(
        form.source_url.as_deref().unwrap_or(""),
        actor,
        &form.command_key,
    )?;
    let book = new_book_with_taxonomy(
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
    )?;
    let book = with_book_description(book, form.description.as_deref().unwrap_or(""))?;
    let book = with_registration_metadata(
        book,
        RegistrationFields {
            source_url: form.source_url.as_deref().unwrap_or(""),
            original_title: form.original_title.as_deref().unwrap_or(""),
            original_language: form.original_language.as_deref().unwrap_or(""),
            release_year: form.release_year.as_deref().unwrap_or(""),
            project_status: form.project_status.as_deref().unwrap_or(""),
            catalog_section: form.catalog_section.as_deref().unwrap_or(""),
            alt_titles: [
                form.alt_title_1.as_deref().unwrap_or(""),
                form.alt_title_2.as_deref().unwrap_or(""),
                form.alt_title_3.as_deref().unwrap_or(""),
            ],
        },
    )?;
    with_extra_source_links(
        book,
        [
            form.extra_source_1.as_deref().unwrap_or(""),
            form.extra_source_2.as_deref().unwrap_or(""),
        ],
    )
}
