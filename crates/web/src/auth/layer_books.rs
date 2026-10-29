//! D-05/H: owner-only Layer Book picker; no Layer checkout or payment UI.
//! The picker is advisory; D's locked add_book() rechecks membership on POST.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_commerce::{
    domain::layers::{LayerBookCandidatePage, LayerError},
    repositories::layers::PgLayers,
    services::layers::LayerService,
};
use tor_contracts::paid_delivery::PositiveId;

const PAGE_SIZE: u16 = 20;

pub(super) fn routes() -> Router<AuthState> {
    Router::new().route("/me/layers/{layer_id}/books", get(page).post(add))
}

pub(super) fn enabled() -> bool {
    std::env::var("TOR_LAYER_MANAGEMENT_ENABLED").as_deref() == Ok("1")
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PickerQuery {
    after: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AddBookForm {
    book_id: i64,
    csrf_token: Option<String>,
}

pub(super) fn status(error: LayerError) -> StatusCode {
    match error {
        LayerError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        LayerError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        LayerError::Inaccessible => StatusCode::NOT_FOUND,
        LayerError::StaleOffer | LayerError::InsufficientFunds | LayerError::ReplayConflict => {
            StatusCode::CONFLICT
        }
    }
}

fn picker_html(
    layer: i64,
    page: &LayerBookCandidatePage,
    csrf: &str,
) -> axum::response::Html<String> {
    let mut list = String::new();
    if page.books.is_empty() {
        list.push_str(
            "<p class=\"empty-state\">Подходящих книг нет. Добавить можно только книгу с одним активным участником — владельцем.</p>",
        );
    } else {
        list.push_str("<ul class=\"author-book-list\">");
        for book in &page.books {
            write!(
                list,
                "<li><form method=\"post\" action=\"/me/layers/{layer}/books\">
                 <span>{}</span>
                 <input type=\"hidden\" name=\"book_id\" value=\"{}\">
                 <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
                 <button class=\"button button--small\" type=\"submit\">Добавить книгу</button>
                 </form></li>",
                pages::escape_html(&book.title),
                book.book_id,
                pages::escape_html(csrf),
            )
            .expect("write String");
        }
        list.push_str("</ul>");
    }
    if let Some(cursor) = page.next_after {
        write!(
            list,
            "<p><a class=\"text-link\" href=\"/me/layers/{layer}/books?after={cursor}\">Следующие книги</a></p>"
        )
        .expect("write String");
    }
    let content = format!(
        "<section class=\"section auth-section\"><div class=\"container\">
         <div class=\"auth-card author-card\">
         <p class=\"eyebrow\">Подписка Layer</p><h1>Добавить книгу</h1>
         <p>В списке только собственные книги без других активных участников.
         Книги с переводчиками или модераторами, даже с долей 0%, недоступны.
         Перед добавлением состав команды проверяется повторно.</p>
         {list}<p><a href=\"/me/books\">Мои книги</a></p>
         </div></div></section>"
    );
    pages::layout(
        "Книги для Layer",
        "Подбор только личных книг для подписки.",
        &content,
    )
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    Query(query): Query<PickerQuery>,
    headers: HeaderMap,
) -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(error) => return deny(error),
    };
    let (Ok(actor), Ok(layer)) = (PositiveId::new(actor), PositiveId::new(raw_layer)) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let after = match query.after.map(PositiveId::new).transpose() {
        Ok(after) => after,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    let candidates = match service.addable_books(actor, layer, after, PAGE_SIZE).await {
        Ok(candidates) => candidates,
        Err(error) => return deny(status(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(picker_html(layer.get(), &candidates, &csrf), &cookies)
}

pub(super) async fn add(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<AddBookForm>,
) -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Ok(actor), Ok(layer), Ok(book)) = (
        PositiveId::new(actor),
        PositiveId::new(raw_layer),
        PositiveId::new(form.book_id),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    match service.add_book(actor, layer, book).await {
        Ok(_) => private_response(Redirect::to(&format!("/me/layers/{raw_layer}/books")), &[]),
        Err(error) => deny(status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_commerce::domain::layers::LayerBookCandidate;

    #[test]
    fn d_05_picker_escapes_untrusted_book_titles_and_excludes_user_identity() {
        let page = LayerBookCandidatePage {
            books: vec![LayerBookCandidate {
                book_id: 91,
                title: "<script>alert('x')</script>".into(),
            }],
            next_after: Some(91),
        };
        let html = picker_html(9, &page, "test_csrf").0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("name=\"book_id\" value=\"91\""));
        assert!(html.contains("name=\"csrf_token\""));
        assert!(html.contains("books?after=91"));
        assert!(!html.contains("user_id"));
        assert!(!html.contains("share_percent"));
    }

    #[test]
    fn d_05_empty_picker_has_no_add_book_form() {
        let html = picker_html(
            9,
            &LayerBookCandidatePage {
                books: vec![],
                next_after: None,
            },
            "token",
        )
        .0;
        assert!(html.contains("Подходящих книг нет"));
        assert!(!html.contains("name=\"book_id\""));
    }
}
