//! S020: active Book member origin language, release year and catalog section.
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
use tor_books::{repositories::postgres::ManagedOriginMetadata, services::authoring::AuthorError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OriginForm {
    original_language: String,
    release_year: String,
    catalog_section: String,
    expected_revision: i64,
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/me/books/{id}/settings/origin", get(edit).post(save))
}

fn render(book_id: i64, book: &ManagedOriginMetadata, csrf: &str) -> Html<String> {
    let year = book
        .release_year
        .map(|value| value.to_string())
        .unwrap_or_default();
    let language = pages::escape_html(&book.original_language);
    let mut sections = String::new();
    for (code, name) in [
        ("", "Выбрать позже"),
        ("novels/chinese", "Китайские новеллы"),
        ("novels/korean", "Корейские новеллы"),
        ("novels/japanese", "Японские новеллы"),
        ("novels/english", "Английские новеллы"),
        ("novels/original", "Авторские произведения"),
        ("fanfic/translated", "Переводы фанфиков"),
    ] {
        let selected = if book.catalog_section == code {
            " selected"
        } else {
            ""
        };
        sections.push_str(&format!(
            "<option value=\"{code}\"{selected}>{name}</option>"
        ));
    }
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-card">
        <h1>Сведения об оригинале «{}»</h1>
        <p>Язык оригинала, год и раздел каталога не меняют язык перевода,
        публикацию, доступ к главам и покупки.</p>
        <form class="auth-form" method="post"
          action="/me/books/{book_id}/settings/origin">
        <input type="hidden" name="csrf_token" value="{}">
        <input type="hidden" name="expected_revision" value="{}">
        <label for="s020-language">Язык оригинала (например, zh, en)</label>
        <input id="s020-language" name="original_language" maxlength="32"
          pattern="[A-Za-z-]*" value="{}">
        <label for="s020-year">Год выпуска</label>
        <input id="s020-year" name="release_year" maxlength="4"
          inputmode="numeric" pattern="[0-9]{{4}}" value="{}">
        <label for="s020-section">Раздел каталога</label>
        <select id="s020-section" name="catalog_section">{sections}</select>
        <button class="button" type="submit">Сохранить сведения</button>
        </form><p><a href="/me/books/{book_id}">Вернуться к книге</a></p>
        </div></div></section>"#,
        pages::escape_html(&book.title),
        pages::escape_html(csrf),
        book.revision,
        language,
        year,
    );
    pages::layout("Сведения об оригинале", "Редактирование книги.", &content)
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
        .manageable_origin_metadata(actor, book_id)
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
    Form(form): Form<OriginForm>,
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
        .update_origin_metadata(
            actor,
            book_id,
            form.expected_revision,
            &form.original_language,
            &form.release_year,
            &form.catalog_section,
        )
        .await
    {
        Ok(_) => private_response(
            Redirect::to(&format!("/me/books/{book_id}/settings/origin")),
            &[],
        ),
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
    fn s020_origin_form_escapes_book_title_and_has_no_publication_switch() {
        let book = ManagedOriginMetadata {
            title: "<script>alert(1)</script>".into(),
            original_language: "zh".into(),
            release_year: Some(2024),
            catalog_section: "novels/chinese".into(),
            revision: 3,
        };
        let page = render(42, &book, "csrf").0;
        assert!(page.contains("&lt;script&gt;"));
        assert!(!page.contains("<script>alert(1)</script>"));
        assert!(page.contains("name=\"expected_revision\" value=\"3\""));
        assert!(page.contains("value=\"novels/chinese\" selected"));
        assert!(!page.contains("name=\"visibility\""));
        assert!(!page.contains("name=\"access_level\""));
        assert!(!page.contains("name=\"target_language\""));
    }
}
