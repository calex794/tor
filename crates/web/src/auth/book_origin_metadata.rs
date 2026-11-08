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
use tor_books::{
    repositories::postgres::{ManagedOriginMetadata, origin_metadata::OriginFields},
    services::authoring::{AuthorError, DiscoveryFields},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OriginForm {
    original_language: String,
    release_year: String,
    catalog_section: String,
    // Old origin-only forms omit these; a complete set opts into the
    // atomic discovery metadata update without losing legacy label values.
    author_name: Option<String>,
    genres: Option<String>,
    tags: Option<String>,
    fandoms: Option<String>,
    expected_revision: i64,
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/me/books/{id}/settings/origin", get(edit).post(save))
        .route("/me/books/{id}/settings", get(index))
        .merge(super::book_lifecycle::router())
        .merge(super::book_access_level::router())
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
        <h2>Автор и темы книги</h2>
        <p>До восьми значений через запятую для каждого поля.
        Названия тем можно менять, не перепубликуя главы.</p>
        <label for="s020-author">Автор произведения</label>
        <input id="s020-author" name="author_name" maxlength="160" value="{}">
        <label for="s020-genres">Жанры</label>
        <input id="s020-genres" name="genres" maxlength="400" value="{}">
        <label for="s020-tags">Теги</label>
        <input id="s020-tags" name="tags" maxlength="400" value="{}">
        <label for="s020-fandoms">Фэндомы</label>
        <input id="s020-fandoms" name="fandoms" maxlength="400" value="{}">
        <button class="button" type="submit">Сохранить сведения</button>
        </form><p><a href="/me/books/{book_id}/settings">Все настройки книги</a></p>
        </div></div></section>"#,
        pages::escape_html(&book.title),
        pages::escape_html(csrf),
        book.revision,
        language,
        year,
        pages::escape_html(&book.author_name),
        pages::escape_html(&book.genres.join(", ")),
        pages::escape_html(&book.tags.join(", ")),
        pages::escape_html(&book.fandoms.join(", ")),
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
    let discovery = match (
        form.author_name.as_deref(),
        form.genres.as_deref(),
        form.tags.as_deref(),
        form.fandoms.as_deref(),
    ) {
        (None, None, None, None) => None,
        (Some(author_name), Some(genres), Some(tags), Some(fandoms)) => Some(DiscoveryFields {
            author_name,
            genres,
            tags,
            fandoms,
        }),
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let result = if let Some(discovery) = discovery {
        state
            .authoring
            .update_origin_and_discovery(
                actor,
                book_id,
                form.expected_revision,
                OriginFields {
                    language: &form.original_language,
                    year: &form.release_year,
                    section: &form.catalog_section,
                },
                discovery,
            )
            .await
    } else {
        state
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
    };
    match result {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{book_id}/settings")), &[]),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Missing | AuthorError::Forbidden) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn render_index(id: i64, title: &str, age_editor_enabled: bool) -> Html<String> {
    let age_link = if age_editor_enabled {
        format!("<li><a href=\"/me/books/{id}/settings/access-level\">Возрастной уровень книги</a></li>")
    } else {
        String::new()
    };
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-card"><h1>Настройки книги «{}»</h1>
        <p>Редактировать эти свойства могут действующие участники данной книги.
        Публикация, платные главы и владение управляются отдельно.</p>
        <ul>
        <li><a href="/me/books/{id}/settings/titles">Названия книги</a></li>
        <li><a href="/me/books/{id}/description">Аннотация книги</a></li>
        <li><a href="/me/books/{id}/settings/sources">Ссылки на источники</a></li>
        <li><a href="/me/books/{id}/settings/origin">Оригинал, автор, жанры, теги и фэндомы</a></li>
        <li><a href="/me/books/{id}/settings/project-status">Состояние работы</a></li>
        <li><a href="/me/books/{id}/settings/related">Связанные книги</a></li>
        <li><a href="/me/books/{id}/settings/lifecycle">Жизненный цикл</a></li>
        <li><a href="/me/books/{id}/settings/ownership">Владелец и доли доходов</a></li>
        {age_link}
        </ul></div></div></section>"#,
        pages::escape_html(title)
    );
    pages::layout("Настройки книги", "Редакторские настройки книги.", &content)
}

pub(super) async fn index(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.manageable_origin_metadata(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    private_response(render_index(id, &book.title, state.age_editor_enabled), &[])
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
            author_name: "Автор \"<Script>\"".into(),
            genres: vec!["фэнтези".into(), "магия".into()],
            tags: vec!["перевод".into()],
            fandoms: vec!["наруто".into()],
            revision: 3,
        };
        let page = render(42, &book, "csrf").0;
        assert!(page.contains("&lt;script&gt;"));
        assert!(!page.contains("<script>alert(1)</script>"));
        assert!(page.contains("name=\"expected_revision\" value=\"3\""));
        assert!(page.contains("value=\"novels/chinese\" selected"));
        assert!(page.contains("name=\"author_name\""));
        assert!(page.contains("name=\"genres\""));
        assert!(page.contains("name=\"tags\""));
        assert!(page.contains("name=\"fandoms\""));
        assert!(page.contains("&quot;&lt;Script&gt;&quot;"));
        assert!(!page.contains("\"<Script>\""));
        assert!(!page.contains("name=\"visibility\""));
        assert!(!page.contains("name=\"access_level\""));
        assert!(!page.contains("name=\"target_language\""));
    }

    #[test]
    fn s020_member_index_has_safe_links_and_escaped_title() {
        let html = render_index(42, "<script>unsafe</script>", false).0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        for path in [
            "/me/books/42/settings/titles",
            "/me/books/42/description",
            "/me/books/42/settings/sources",
            "/me/books/42/settings/origin",
            "/me/books/42/settings/project-status",
            "/me/books/42/settings/related",
            "/me/books/42/settings/lifecycle",
        ] {
            assert!(html.contains(path));
        }
        assert!(!html.contains("/me/books/42/publish"));
        assert!(!html.contains("/me/books/42/chapters"));
        assert!(!html.contains("/me/books/42/settings/access-level"));
        let enabled = render_index(42, "Safe title", true).0;
        assert!(enabled.contains("/me/books/42/settings/access-level"));
    }
}
