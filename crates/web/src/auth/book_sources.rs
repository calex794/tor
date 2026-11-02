//! Owner-only editor of the original URL and two optional source links.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
};
use serde::Deserialize;
use tor_books::{repositories::postgres::OwnedBookSourceLinks, services::authoring::AuthorError};

#[derive(Deserialize)]
pub(super) struct EditSourcesForm {
    source_url: String,
    extra_source_1: String,
    extra_source_2: String,
    expected_revision: i64,
    csrf_token: Option<String>,
}

fn render(book_id: i64, book: &OwnedBookSourceLinks, csrf: &str) -> Html<String> {
    let extra1 = book.extra.first().map(String::as_str).unwrap_or("");
    let extra2 = book.extra.get(1).map(String::as_str).unwrap_or("");
    let content = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-card">
        <p class="eyebrow">Кабинет автора</p>
        <h1>Ссылки на книгу «{}»</h1>
        <p>Укажите первоисточник на языке автора и, при необходимости, ссылки на перевод или зеркало. Разрешены ссылки с разных сайтов.</p>
        <form class="auth-form" method="post" action="/me/books/{book_id}/settings/sources">
        <input type="hidden" name="csrf_token" value="{}">
        <input type="hidden" name="expected_revision" value="{}">
        <label for="source-original">Ссылка на оригинал</label>
        <input id="source-original" name="source_url" type="url" maxlength="512" value="{}">
        <label for="source-extra-1">Дополнительная ссылка на произведение</label>
        <input id="source-extra-1" name="extra_source_1" type="url" maxlength="512" value="{}">
        <label for="source-extra-2">Ещё одна ссылка</label>
        <input id="source-extra-2" name="extra_source_2" type="url" maxlength="512" value="{}">
        <p>Чтобы удалить дополнительную ссылку, очистите её поле. При конфликте с другой книгой изменения не сохраняются.</p>
        <button class="button" type="submit">Сохранить ссылки</button>
        </form>
        <p><a href="/me/books/{book_id}">Вернуться к книге</a></p>
        </div></div></section>"#,
        pages::escape_html(&book.title),
        pages::escape_html(csrf),
        book.revision,
        pages::escape_html(&book.primary),
        pages::escape_html(extra1),
        pages::escape_html(extra2),
    );
    pages::layout(
        "Ссылки книги",
        "Редактирование известных источников книги.",
        &content,
    )
}

fn problem(book_id: i64, code: StatusCode) -> Response {
    let (title, explanation) = if code == StatusCode::CONFLICT {
        (
            "Ссылки не сохранены",
            "Эта ссылка уже используется другой книгой либо данные изменились в другой вкладке. Проверьте адреса и откройте форму заново.",
        )
    } else {
        (
            "Неверная ссылка",
            "Укажите обычный полный адрес HTTP или HTTPS без внутренних адресов, учётных данных и запрещённых символов.",
        )
    };
    private_response(
        (
            code,
            pages::layout(
                title,
                explanation,
                &format!(
                    r#"<section class="section auth-section"><div class="container"><div class="auth-card">
                    <h1>{title}</h1><p>{explanation}</p>
                    <p><a href="/me/books/{book_id}/settings/sources">Открыть ссылки заново</a></p>
                    </div></div></section>"#
                ),
            ),
        ),
        &[],
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
    let book = match state
        .authoring
        .owned_book_source_links(actor, book_id)
        .await
    {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(render(book_id, &book, &csrf), &cookies)
}

pub(super) async fn save(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<EditSourcesForm>,
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
    match state
        .authoring
        .update_book_source_links(
            actor,
            book_id,
            form.expected_revision,
            [&form.source_url, &form.extra_source_1, &form.extra_source_2],
        )
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{book_id}")), &[]),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Conflict) => problem(book_id, StatusCode::CONFLICT),
        Err(AuthorError::Invalid) => problem(book_id, StatusCode::BAD_REQUEST),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s019_source_editor_escapes_existing_urls_and_titles() {
        let book = OwnedBookSourceLinks {
            title: "<img src=x onerror=alert(1)>".to_owned(),
            primary: "https://example.org/a?x=\" onfocus=\"evil".to_owned(),
            extra: vec!["https://example.cn/?a=1&b=2".to_owned()],
            revision: 7,
        };
        let html = render(123, &book, "csrf").0;
        assert!(!html.contains("<img src=x"));
        assert!(!html.contains("onfocus=\"evil"));
        assert!(html.contains("&amp;b=2"));
        assert!(html.contains("name=\"expected_revision\" value=\"7\""));
    }
}
