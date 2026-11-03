//! A-05 / NEwBOO moderation: bounded book queue and reversible hold only.
//! Never publish a draft, grant paid chapter access, or edit owner metadata.
use super::{deny, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tor_accounts::{
    repositories::moderation_books::{ModerationBook, ModerationBooksError},
    services::security::session_token_digest,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    before: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HoldForm {
    csrf_token: String,
    expected_hold: bool,
    new_hold: bool,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/moderation/books", get(page))
        .route("/moderation/books/{id}/hold", post(submit))
        .merge(super::moderation_chapters::router())
}

pub(super) fn error_code(error: ModerationBooksError) -> StatusCode {
    match error {
        ModerationBooksError::Denied => StatusCode::FORBIDDEN,
        ModerationBooksError::Missing => StatusCode::NOT_FOUND,
        ModerationBooksError::Invalid => StatusCode::BAD_REQUEST,
        ModerationBooksError::Stale => StatusCode::CONFLICT,
        ModerationBooksError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn render(rows: &[ModerationBook], csrf: &str) -> String {
    let mut list = String::new();
    for row in rows {
        let title = pages::escape_html(&row.title);
        let current = if row.admin_hold {
            "Удерживается модерацией"
        } else {
            "Без удержания"
        };
        let action = if row.admin_hold {
            "Снять удержание"
        } else {
            "Удержать"
        };
        let next = !row.admin_hold;
        let owner = row
            .owner_user_id
            .map_or_else(|| "—".to_owned(), |id| id.to_string());
        list.push_str(&format!(
            r#"<article class="me-now-card">
               <h2>{title}</h2><p>Книга #{id} · Владелец #{owner} ·
               Публикация: {visibility} · {current}</p>
               <form method="post" action="/moderation/books/{id}/hold">
               <input type="hidden" name="csrf_token" value="{csrf}">
               <input type="hidden" name="expected_hold" value="{held}">
               <input type="hidden" name="new_hold" value="{next}">
               <button type="submit">{action}</button>
               </form></article>"#,
            id = row.id,
            visibility = pages::escape_html(&row.visibility),
            held = row.admin_hold,
        ));
    }
    if rows.is_empty() {
        list.push_str("<p>Книги отсутствуют.</p>");
    }
    if rows.len() == 40 {
        if let Some(last) = rows.last() {
            list.push_str(&format!(
                r#"<a href="/moderation/books?before={}">Следующая страница</a>"#,
                last.id
            ));
        }
    }
    pages::layout(
        "Модерация книг",
        "Контроль модерационных удержаний без доступа к покупкам.",
        &format!(
            r#"<section class="me-page"><div class="container">
              <h1>Модерация книг</h1>
              <p>Удержание не меняет авторскую публикацию и не создаёт
              административное право на платные главы.</p>
              {list}</div></section>"#
        ),
    )
    .0
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let rows = match state.accounts.moderation_books(&digest, query.before).await {
        Ok(rows) => rows,
        Err(error) => return deny(error_code(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(render(&rows, &pages::escape_html(&csrf)), &cookies);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<HoldForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    match state
        .accounts
        .moderate_book_hold(&digest, id, form.expected_hold, form.new_hold)
        .await
    {
        Ok(()) => private_response(Redirect::to("/moderation/books"), &[]),
        Err(error) => deny(error_code(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moderation_list_escapes_private_titles_and_keeps_author_publication() {
        let rows = vec![ModerationBook {
            id: 42,
            title: "<script>private</script>".into(),
            visibility: "draft".into(),
            admin_hold: false,
            owner_user_id: None,
        }];
        let html = render(&rows, "csrf");
        assert!(html.contains("&lt;script&gt;private&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("value=\"draft\""));
        assert!(html.contains("name=\"new_hold\" value=\"true\""));
        assert!(!html.contains("name=\"visibility\""));
        assert!(!html.contains("purchase_id"));
    }
}
