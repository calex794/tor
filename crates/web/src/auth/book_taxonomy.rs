//! B/TAX-02 owner-only metadata edit page, rev-fenced POST.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_books::{
    repositories::postgres::taxonomy_edit::{TaxonomyEntry, TaxonomySnapshot},
    services::authoring::{AuthorError, TaxonomySelection},
};

#[derive(Deserialize)]
pub(super) struct TaxonomyEditForm {
    genre_ids: Option<String>,
    tag_ids: Option<String>,
    fandom_ids: Option<String>,
    taxonomy_revision: i64,
    csrf_token: Option<String>,
}

fn chips(entries: &[TaxonomyEntry]) -> String {
    let mut text = String::new();
    for term in entries {
        let label = pages::escape_html(&term.label);
        write!(
            text,
            "<button type=\"button\" class=\"taxonomy-picker__chip\" \
             data-taxonomy-id=\"{}\" data-taxonomy-label=\"{}\">{} ×</button>",
            term.id, label, label
        )
        .expect("write taxonomy chip");
    }
    text
}

fn ids(entries: &[TaxonomyEntry]) -> String {
    entries
        .iter()
        .map(|term| term.id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn page_html(
    book_id: i64,
    title: &str,
    csrf: &str,
    snapshot: &TaxonomySnapshot,
) -> axum::response::Html<String> {
    if !snapshot.editable {
        let raw = |values: &[String]| {
            if values.is_empty() {
                "—".to_owned()
            } else {
                pages::escape_html(&values.join(", "))
            }
        };
        let content = format!(
            "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card author-card\">\
             <h1>Теги книги «{}» требуют сопоставления</h1>\
             <p>Для этой книги сохранились старые метки, которых нет в действующем справочнике, \
             или одна из меток заблокирована. Автоматическое сохранение удалило бы эти данные, \
             поэтому редактирование пока запрещено. Сначала требуется сопоставление редактором справочника.</p>\
             <p>Жанры: {}</p><p>Теги: {}</p><p>Фэндомы: {}</p>\
             <p><a href=\"/me/books/{book_id}\">Вернуться к книге</a></p>\
             </div></div></section>",
            pages::escape_html(title),
            raw(&snapshot.raw_genres), raw(&snapshot.raw_tags), raw(&snapshot.raw_fandoms)
        );
        return pages::layout("Теги книги", "Требуется сопоставление меток.", &content);
    }
    let html = include_str!("../../templates/author_edit_taxonomy.html")
        .replace("<!--BOOK_ID-->", &book_id.to_string())
        .replace("<!--BOOK_TITLE-->", &pages::escape_html(title))
        .replace("<!--CSRF_TOKEN-->", &pages::escape_html(csrf))
        .replace("<!--REVISION-->", &snapshot.revision.to_string())
        .replace("<!--GENRE_CHIPS-->", &chips(&snapshot.genres))
        .replace("<!--GENRE_IDS-->", &ids(&snapshot.genres))
        .replace("<!--TAG_CHIPS-->", &chips(&snapshot.tags))
        .replace("<!--TAG_IDS-->", &ids(&snapshot.tags))
        .replace("<!--FANDOM_CHIPS-->", &chips(&snapshot.fandoms))
        .replace("<!--FANDOM_IDS-->", &ids(&snapshot.fandoms));
    pages::layout(
        "Теги книги",
        "Редактирование жанров, тегов и фэндомов.",
        &html,
    )
}

pub(super) async fn edit(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.owned_book(actor, book_id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let snapshot = match state.authoring.taxonomy_snapshot(actor, book_id).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(page_html(book_id, &book.title, &csrf, &snapshot), &cookies)
}

pub(super) async fn save(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<TaxonomyEditForm>,
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
    let selection = match TaxonomySelection::parse(
        form.genre_ids.as_deref(),
        form.tag_ids.as_deref(),
        form.fandom_ids.as_deref(),
    ) {
        Ok(selection) => selection,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    match state
        .authoring
        .update_book_taxonomy(actor, book_id, form.taxonomy_revision, &selection)
        .await
    {
        Ok(()) => private_response(Redirect::to(&format!("/me/books/{book_id}")), &[]),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
