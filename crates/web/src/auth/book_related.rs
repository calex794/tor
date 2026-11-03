//! S020: NEwBOO-style reciprocal related-book management.
//! Search is full main-title equality only; no private candidate information.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tor_books::{
    repositories::postgres::{OwnedRelatedBooks, RelatedBookCard},
    services::authoring::AuthorError,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchForm {
    title: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkForm {
    target_id: i64,
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/me/books/{id}/settings/related", get(page))
        .route("/me/books/{id}/settings/related/search", post(search))
        .route("/me/books/{id}/settings/related/add", post(add))
        .route("/me/books/{id}/settings/related/remove", post(remove))
}

fn error_response(error: AuthorError) -> Response {
    match error {
        AuthorError::Missing | AuthorError::Forbidden => deny(StatusCode::NOT_FOUND),
        AuthorError::Invalid => deny(StatusCode::BAD_REQUEST),
        AuthorError::Conflict => deny(StatusCode::CONFLICT),
        AuthorError::Unavailable => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn render(
    id: i64,
    book: &OwnedRelatedBooks,
    csrf: &str,
    candidates: &[RelatedBookCard],
    info: &str,
) -> Html<String> {
    let mut current = String::new();
    for link in &book.linked {
        current.push_str(&format!(
            r#"<li><a href="/books/{target}">{title}</a>
              <form method="post" action="/me/books/{id}/settings/related/remove">
              <input type="hidden" name="csrf_token" value="{csrf}">
              <input type="hidden" name="target_id" value="{target}">
              <button type="submit">Удалить связь</button></form></li>"#,
            target = link.id,
            title = pages::escape_html(&link.title),
            csrf = pages::escape_html(csrf),
        ));
    }
    if current.is_empty() {
        current.push_str("<li>Связанных публичных книг пока нет.</li>");
    }
    let mut options = String::new();
    for link in candidates {
        options.push_str(&format!(
            r#"<li>{title}
                <form method="post" action="/me/books/{id}/settings/related/add">
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="target_id" value="{target}">
                <button type="submit">Связать</button></form></li>"#,
            title = pages::escape_html(&link.title),
            csrf = pages::escape_html(csrf),
            target = link.id,
        ));
    }
    let options = if options.is_empty() {
        String::new()
    } else {
        format!("<h2>Точные совпадения</h2><ul>{options}</ul>")
    };
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
         <div class="auth-card author-card">
         <h1>Связанные книги: {title}</h1>
         <p>Связи двусторонние. Добавлять можно только существующие публичные книги.
         Скрытые и заблокированные книги никогда не появляются среди кандидатов.</p>
         <h2>Связи</h2><ul>{current}</ul>
         <form method="post" class="auth-form"
               action="/me/books/{id}/settings/related/search">
         <input type="hidden" name="csrf_token" value="{csrf}">
         <label for="related-name">Полное название публичной книги</label>
         <input id="related-name" name="title" maxlength="200" required>
         <button type="submit">Найти по названию</button>
         </form><p>{info}</p>{options}
         <p><a href="/me/books/{id}">Вернуться к книге</a></p>
         </div></div></section>"#,
        title = pages::escape_html(&book.title),
        csrf = pages::escape_html(csrf),
        info = pages::escape_html(info),
    );
    pages::layout("Связанные книги", "Управление связанными книгами", &content)
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.owned_related_books(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return error_response(error),
    };
    let (csrf, set_cookies) = state.cookies.csrf_for_page(&headers);
    private_response(render(id, &book, &csrf, &[], ""), &set_cookies)
}

pub(super) async fn search(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<SearchForm>,
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
    let book = match state.authoring.owned_related_books(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return error_response(error),
    };
    let matches = match state
        .authoring
        .find_related_candidates(actor, id, &form.title)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return error_response(error),
    };
    if matches.len() == 1 {
        if let Err(error) = state.authoring.add_related_book(actor, id, matches[0].id).await {
            return error_response(error);
        }
        return private_response(
            Redirect::to(&format!("/me/books/{id}/settings/related")),
            &[],
        );
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let info = if matches.is_empty() {
        "Книга с таким полным названием не найдена."
    } else {
        "Найдено несколько книг с одинаковым названием. Выберите нужную."
    };
    private_response(render(id, &book, &csrf, &matches, info), &cookies)
}

pub(super) async fn add(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<LinkForm>,
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
    if let Err(error) = state.authoring.add_related_book(actor, id, form.target_id).await {
        return error_response(error);
    }
    private_response(
        Redirect::to(&format!("/me/books/{id}/settings/related")),
        &[],
    )
}

pub(super) async fn remove(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<LinkForm>,
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
    if let Err(error) = state
        .authoring
        .remove_related_book(actor, id, form.target_id)
        .await
    {
        return error_response(error);
    }
    private_response(
        Redirect::to(&format!("/me/books/{id}/settings/related")),
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s020_related_book_page_never_interpolates_private_html() {
        let example = OwnedRelatedBooks {
            title: "<script>unsafe</script>".to_owned(),
            linked: vec![RelatedBookCard {
                id: 52,
                title: "<img src=x onerror=alert(1)>".to_owned(),
            }],
        };
        let html = render(17, &example, "secret", &[], "").0;
        assert!(html.contains("&lt;script&gt;unsafe&lt;/script&gt;"));
        assert!(html.contains("&lt;img"));
        assert!(!html.contains("<img"));
        assert!(html.contains("related/remove"));
    }
}
