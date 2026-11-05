//! S020/P0: age-tier settings are mounted only behind an explicit
//! disabled-by-default acceptance flag. B enforces monotonic transitions.
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
use tor_books::{repositories::postgres::ManagedBookAccessLevel, services::authoring::AuthorError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AccessForm {
    expected_level: String,
    requested_level: String,
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/me/books/{id}/settings/access-level", get(edit).post(save))
}

fn render(book_id: i64, book: &ManagedBookAccessLevel, csrf: &str) -> Html<String> {
    let editor = if let Some(current) = book.level.as_deref() {
        let tiers = [
            ("basic", "Без возрастных ограничений"),
            ("adult", "18+"),
            ("pro_adult", "21+"),
        ];
        let index = tiers.iter().position(|(tier, _)| *tier == current);
        if let Some(index) = index {
            let mut choices = String::new();
            for (value, label) in tiers.iter().skip(index) {
                let selected = if *value == current { " selected" } else { "" };
                choices.push_str(&format!(
                    "<option value=\"{value}\"{selected}>{label}</option>"
                ));
            }
            format!(
                r#"<form class="auth-form" method="post"
                       action="/me/books/{book_id}/settings/access-level">
                    <input type="hidden" name="csrf_token" value="{}">
                    <input type="hidden" name="expected_level" value="{current}">
                    <label for="book-age-tier">Возрастной уровень книги</label>
                    <select id="book-age-tier" name="requested_level">{choices}</select>
                    <button class="button" type="submit">Повысить уровень</button>
                    </form>"#,
                pages::escape_html(csrf),
            )
        } else {
            String::from("<p>Неизвестный уровень: изменение заблокировано.</p>")
        }
    } else {
        String::from(
            "<p>Неаттестованный уровень старой книги. Назначать basic автоматически запрещено.</p>",
        )
    };
    pages::layout(
        "Возрастной уровень книги",
        "Настройки защиты доступа 18+/21+.",
        &format!(
            r#"<section class="section auth-section"><div class="container">
               <div class="auth-card author-card">
               <h1>Возрастной уровень книги «{}»</h1>
               <p>Уровень разрешено только повышать. Для первого перехода на 21+
               требуется действующий личный допуск. Покупки, публикация и редакционный
               статус остаются без изменений.</p>
               {editor}
               <p><a href="/me/books/{book_id}/settings">Все настройки книги</a></p>
               </div></div></section>"#,
            pages::escape_html(&book.title),
        ),
    )
}

pub(super) async fn edit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !state.age_editor_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.manageable_access_level(actor, id).await {
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
    Form(form): Form<AccessForm>,
) -> Response {
    if !state.age_editor_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
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
        .verified_upgrade_book_access_level(actor, id, &form.expected_level, &form.requested_level)
        .await
    {
        Ok(()) => private_response(
            Redirect::to(&format!("/me/books/{id}/settings/access-level")),
            &[],
        ),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Missing | AuthorError::Forbidden) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s020_age_editor_is_monotonic_escaped_and_unknown_tier_read_only() {
        let basic = ManagedBookAccessLevel {
            title: "<script>alert(1)</script>".to_owned(),
            level: Some("basic".to_owned()),
        };
        let html = render(42, &basic, "csrf").0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("name=\"expected_level\" value=\"basic\""));
        assert!(html.contains("value=\"adult\""));
        assert!(html.contains("value=\"pro_adult\""));
        assert!(!html.contains("name=\"visibility\""));
        let adult = ManagedBookAccessLevel {
            level: Some("adult".to_owned()),
            ..basic
        };
        let html = render(42, &adult, "csrf").0;
        assert!(!html.contains("value=\"basic\""));
        assert!(html.contains("value=\"pro_adult\""));
        let unknown = ManagedBookAccessLevel {
            level: None,
            ..adult
        };
        let html = render(42, &unknown, "csrf").0;
        assert!(!html.contains("method=\"post\""));
    }
}
