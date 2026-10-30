//! E-03: private Inbox browser and JSON access to canonical received replies.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tor_discussions::{
    repositories::{inbox::PgInbox, public_book_comments::ListCommentsError},
    services::inbox::{comment_url, InboxFilter, InboxPage, InboxService},
};

#[derive(Default, Deserialize)]
pub(super) struct InboxQuery {
    filter: Option<String>,
    after: Option<i64>,
    limit: Option<u16>,
}

async fn load(state: &AuthState, actor: i64, query: InboxQuery) -> Result<InboxPage, StatusCode> {
    let filter =
        InboxFilter::parse(query.filter.as_deref()).map_err(|_| StatusCode::BAD_REQUEST)?;
    InboxService::new(PgInbox::new(state.accounts.pool().clone()))
        .list(actor, filter, query.after, query.limit.unwrap_or(25))
        .await
        .map_err(|err| match err {
            ListCommentsError::Invalid => StatusCode::BAD_REQUEST,
            ListCommentsError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<InboxQuery>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let page = match load(&state, actor, query).await {
        Ok(page) => page,
        Err(status) => return deny(status),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(render(&page, &csrf), &cookies)
}

#[derive(Serialize)]
struct ApiItem {
    id: String,
    kind: &'static str,
    created_at: String,
    book_id: String,
    book_title: String,
    url: String,
    is_read: bool,
}
#[derive(Serialize)]
struct ApiPage {
    items: Vec<ApiItem>,
    next_cursor: Option<String>,
}

pub(super) async fn api(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<InboxQuery>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let page = match load(&state, actor, query).await {
        Ok(page) => page,
        Err(status) => return deny(status),
    };
    let response = ApiPage {
        items: page
            .items
            .iter()
            .map(|item| ApiItem {
                id: item.reply.comment_id.to_string(),
                kind: "reply",
                created_at: item.created_at.clone(),
                book_id: item.reply.book_id.to_string(),
                book_title: item.reply.book_title.clone(),
                url: comment_url(item.reply.book_id, item.reply.comment_id),
                is_read: item.reply.is_read,
            })
            .collect(),
        next_cursor: page.next_after.map(|n| n.to_string()),
    };
    private_response(Json(response).into_response(), &[])
}

fn render(page: &InboxPage, csrf: &str) -> axum::response::Html<String> {
    let mut cards = String::new();
    for item in &page.items {
        let r = &item.reply;
        let url = comment_url(r.book_id, r.comment_id);
        let label = if r.is_read {
            "Прочитано"
        } else {
            "Не прочитано"
        };
        let next = if r.is_read { "unread" } else { "read" };
        let button = if r.is_read {
            "Отметить непрочитанным"
        } else {
            "Отметить прочитанным"
        };
        cards.push_str(&format!(
            "<article class=\"me-now-card\"><a class=\"me-now-item\" href=\"{}\"><strong>Ответ в книге «{}»</strong> — {label}</a><form method=\"post\" action=\"/me/replies/{}/read-state\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"state\" value=\"{next}\"><input type=\"hidden\" name=\"return_to\" value=\"inbox\"><button class=\"ui-button ui-button--ghost ui-button--small\" type=\"submit\">{button}</button></form></article>",
            pages::escape_html(&url), pages::escape_html(&r.book_title),
            r.comment_id, pages::escape_html(csrf),
        ));
    }
    if cards.is_empty() {
        cards.push_str("<p class=\"ui-section-header__text\">Ответов пока нет.</p>");
    }
    let next_link = page.next_after.map_or_else(String::new, |after| {
        format!("<a class=\"button\" href=\"/me/inbox?filter={}&amp;after={after}\">Следующие ответы</a>", page.filter.as_str())
    });
    let body = format!(
        "<section class=\"me-page\"><div class=\"container\"><nav class=\"dashboard-nav\" aria-label=\"Разделы кабинета\"><a class=\"dashboard-nav__item\" href=\"/me\">Обзор</a><a class=\"dashboard-nav__item is-active\" href=\"/me/inbox\" aria-current=\"page\">Обсуждения</a><a class=\"dashboard-nav__item\" href=\"/me/library\">Библиотека</a></nav><header class=\"ui-page-header\"><div><p class=\"ui-page-header__kicker\">Личный кабинет</p><h1 class=\"ui-page-header__title\">Входящие обсуждения</h1><p class=\"ui-page-header__text\">Ответы на ваши комментарии в доступных книгах.</p></div></header><nav aria-label=\"Фильтр ответов\"><a href=\"/me/inbox?filter=all\">Все</a> · <a href=\"/me/inbox?filter=unread\">Непрочитанные</a> · <a href=\"/me/inbox?filter=read\">Прочитанные</a></nav><section class=\"me-section\">{cards}{next_link}</section></div></section>"
    );
    pages::layout(
        "Входящие обсуждения",
        "Полученные ответы на комментарии",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_03_inbox_escapes_book_titles_and_renders_read_controls() {
        let item = tor_discussions::repositories::inbox::InboxEntry {
            reply: tor_discussions::repositories::public_book_comments::ReceivedReply {
                comment_id: 26,
                book_id: 4,
                book_title: "<script>secret</script>".into(),
                is_read: false,
            },
            created_at: "2026-10-30T00:00:00Z".into(),
        };
        let page = InboxPage {
            items: vec![item],
            next_after: Some(26),
            filter: InboxFilter::All,
        };
        let html = render(&page, "csrf").0;
        assert!(html.contains("&lt;script&gt;secret&lt;/script&gt;"));
        assert!(!html.contains("<script>secret</script>"));
        assert!(html.contains("/books/4/comments?after=27#comment-26"));
        assert!(html.contains("Отметить прочитанным"));
        assert!(html.contains("after=26"));
    }
}
