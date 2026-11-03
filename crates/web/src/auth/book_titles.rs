//! S020 owner-only names editor: primary, original and three alternatives.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_books::{repositories::postgres::OwnedBookTitles, services::authoring::AuthorError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TitlesForm {
    title: String,
    original_title: String,
    alt_title_1: String,
    alt_title_2: String,
    alt_title_3: String,
    expected_revision: i64,
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/me/books/{id}/settings/titles", get(edit).post(save))
}

fn render(id: i64, book: &OwnedBookTitles, csrf: &str) -> Html<String> {
    let alt = |index: usize| book.alternatives.get(index).map(String::as_str).unwrap_or("");
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-card">
        <h1>Названия книги «{}»</h1>
        <p>Изменения проверяются на повторы во всех названиях книг с тем же языком перевода.
        Совпадение с чужой закрытой книгой не раскрывает её данные.</p>
        <form class="auth-form" method="post" action="/me/books/{id}/settings/titles">
        <input type="hidden" name="csrf_token" value="{}">
        <input type="hidden" name="expected_revision" value="{}">
        <label for="s020-title">Основное название</label>
        <input id="s020-title" name="title" maxlength="200" required value="{}">
        <label for="s020-original">Оригинальное название</label>
        <input id="s020-original" name="original_title" maxlength="200" value="{}">
        <label for="s020-alt1">Альтернативное название 1 (например, английское)</label>
        <input id="s020-alt1" name="alt_title_1" maxlength="200" value="{}">
        <label for="s020-alt2">Альтернативное название 2</label>
        <input id="s020-alt2" name="alt_title_2" maxlength="200" value="{}">
        <label for="s020-alt3">Альтернативное название 3</label>
        <input id="s020-alt3" name="alt_title_3" maxlength="200" value="{}">
        <p>Здесь меняются только названия, не публикация, покупки или ссылки на источник.</p>
        <button class="button" type="submit">Сохранить названия</button>
        </form><p><a href="/me/books/{id}">Вернуться к книге</a></p>
        </div></div></section>"#,
        pages::escape_html(&book.title),
        pages::escape_html(csrf),
        book.revision,
        pages::escape_html(&book.title),
        pages::escape_html(&book.original_title),
        pages::escape_html(alt(0)),
        pages::escape_html(alt(1)),
        pages::escape_html(alt(2)),
    );
    pages::layout("Названия книги", "Редактирование названий.", &content)
}

pub(super) async fn edit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.owned_book_titles(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(render(id, &book, &csrf), &cookies)
}

pub(super) async fn save(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<TitlesForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state.authoring.update_book_titles(
        actor, id, form.expected_revision, &form.title, &form.original_title,
        [&form.alt_title_1, &form.alt_title_2, &form.alt_title_3],
    ).await {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Missing | AuthorError::Forbidden) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s020_names_form_escapes_injection_and_keeps_purchases_untouched() {
        let b = OwnedBookTitles {
            title: "<script>bad</script>".into(), original_title: "修仙之路".into(),
            alternatives: vec!["<img onerror=alert(1)>".into()],
            revision: 3,
        };
        let html = render(42, &b, "csrf").0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>bad</script>"));
        assert!(html.contains("&lt;img"));
        assert!(html.contains("name=\"expected_revision\" value=\"3\""));
        assert!(!html.contains("name=\"visibility\""));
        assert!(!html.contains("name=\"access_level\""));
    }
}
