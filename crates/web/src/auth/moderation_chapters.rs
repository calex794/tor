//! A-05/MOD-C01: NEwBOO-style chapter moderation metadata and reversible holds.
//! Never publish on behalf of an author, show chapter bytes or change paid grants.
use super::{deny, moderation_books::error_code, AuthState};
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
    repositories::moderation_chapters::ModerationChapter,
    services::security::session_token_digest,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    before: Option<i64>,
    status: Option<String>,
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
        .route("/moderation/chapters", get(page))
        .route("/moderation/chapters/{id}/hold", post(submit))
}

fn render(rows: &[ModerationChapter], csrf: &str, status: &str) -> String {
    let mut cards = String::new();
    for row in rows {
        let title = pages::escape_html(&row.title);
        let book = pages::escape_html(&row.book_title);
        let visibility = pages::escape_html(&row.visibility);
        let access = pages::escape_html(&row.access_level);
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
        cards.push_str(&format!(
            r#"<article class="me-now-card">
                <h2>{title}</h2>
                <p>{book} · Книга #{book_id} · Глава #{no} · {visibility} · {access}
                · {current} · Версия: {version} · Публикация: {published}</p>
                <form method="post" action="/moderation/chapters/{id}/hold">
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="expected_hold" value="{held}">
                <input type="hidden" name="new_hold" value="{next}">
                <button type="submit">{action}</button>
                </form></article>"#,
            book_id = row.book_id,
            no = row.chapter_no,
            version = if row.has_version { "есть" } else { "нет" },
            published = if row.has_publication { "есть" } else { "нет" },
            id = row.id,
            held = row.admin_hold,
            next = !row.admin_hold,
        ));
    }
    if rows.is_empty() {
        cards.push_str("<p>Главы отсутствуют.</p>");
    }
    if rows.len() == 40 {
        if let Some(last) = rows.last() {
            cards.push_str(&format!(
                r#"<a href="/moderation/chapters?status={status}&amp;before={}">Следующая страница</a>"#,
                last.id
            ));
        }
    }
    let filters = [
        ("all", "Все"),
        ("hold", "На удержании"),
        ("draft", "Черновики"),
        ("published", "Опубликованные"),
        ("no_version", "Без версии"),
    ];
    let mut links = String::new();
    for (key, caption) in filters {
        links.push_str(&format!(
            r#"<a href="/moderation/chapters?status={key}">{caption}</a> "#,
        ));
    }
    pages::layout(
        "Модерация глав",
        "Контроль глав без доступа к платному или неопубликованному тексту.",
        &format!(
            r#"<section class="me-page"><div class="container">
                <h1>Модерация глав</h1><nav>{links}</nav>
                <p>Модерационное удержание не меняет редакцию,
                публикацию, цену или покупку главы.</p>{cards}
               </div></section>"#
        ),
    )
    .0
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let status = query.status.as_deref().unwrap_or("all");
    if !matches!(status, "all" | "hold" | "draft" | "published" | "no_version") {
        return deny(StatusCode::BAD_REQUEST);
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
    let rows = match state
        .accounts
        .moderation_chapters(&digest, query.before, status)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(error_code(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(render(&rows, &pages::escape_html(&csrf), status), &cookies);
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
        .moderate_chapter_hold(&digest, id, form.expected_hold, form.new_hold)
        .await
    {
        Ok(()) => private_response(Redirect::to("/moderation/chapters"), &[]),
        Err(error) => deny(error_code(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moderator_html_escapes_chapter_and_book_titles_without_versions_or_purchase_payloads() {
        let rows = vec![ModerationChapter {
            id: 17,
            book_id: 3,
            book_title: "<book>".into(),
            chapter_no: 2,
            title: "<script>bad</script>".into(),
            visibility: "draft".into(),
            access_level: "paid".into(),
            admin_hold: false,
            has_publication: false,
            has_version: true,
        }];
        let html = render(&rows, "csrf", "draft");
        assert!(html.contains("&lt;book&gt;"));
        assert!(html.contains("&lt;script&gt;bad&lt;/script&gt;"));
        assert!(!html.contains("<script>bad</script>"));
        assert!(!html.contains("body_markdown"));
        assert!(!html.contains("purchase_id"));
        assert!(!html.contains("name=\"visibility\""));
        assert!(!html.contains("name=\"price_amount\""));
        assert!(html.contains("name=\"new_hold\" value=\"true\""));
    }
}
