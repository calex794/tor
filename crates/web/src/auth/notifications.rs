//! On-site notifications: session scope, no-store responses, CSRF on read actions.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_social::repositories::notifications::PgNotifications;

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/notifications", get(page))
        .route("/notifications/{id}/read", post(mark_one))
        .route("/notifications/read-all", post(mark_all))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NotificationQuery {
    before: Option<i64>,
    tab: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadForm {
    csrf_token: String,
}

fn csrf(state: &AuthState, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<NotificationQuery>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    // Only allow known source tabs; never interpolate raw user input into HTML.
    let requested_tab = query.tab.as_deref().unwrap_or_default().trim().to_ascii_lowercase();
    let tab = match requested_tab.as_str() {
        "releases" => "releases",
        "system" => "system",
        "discussions" => "discussions",
        _ => "notifications",
    };
    // E-03 already owns the actor-scoped discussion Inbox; do not duplicate
    // its read/unread state in the mail notification projection.
    if tab == "discussions" {
        return private_response(Redirect::to("/me/inbox"), &[]);
    }
    let store = PgNotifications::new(state.accounts.pool().clone());
    let rows = match store.list_section(actor, query.before, 50, tab).await {
        Ok(rows) => rows,
        Err(e) => return deny(super::mail::status(e)),
    };
    let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
    let csrf_token = pages::escape_html(&csrf_token);
    let mut tab_links = String::new();
    for (value, label, href) in [
        ("notifications", "Все", "/notifications"),
        ("discussions", "Обсуждения", "/me/inbox"),
        ("releases", "Релизы", "/notifications?tab=releases"),
        ("system", "Система", "/notifications?tab=system"),
    ] {
        let active = if value == tab { " is-active" } else { "" };
        let current = if value == tab { " aria-current=\"page\"" } else { "" };
        write!(
            tab_links,
            "<a class=\"ui-button ui-button--small{active}\" href=\"{href}\"{current}>{label}</a>"
        )
        .expect("notification tab");
    }
    let mut cards = String::new();
    for row in &rows {
        let status = if row.is_read {
            "Прочитано"
        } else {
            "Не прочитано"
        };
        write!(
            cards,
            "<article class=\"me-now-card\" data-notification-id=\"{}\" data-notification-kind=\"{}\"><a href=\"{}\"><strong>{}</strong></a><p>{}</p><time>{}</time><p>{status}</p>",
            row.id,
            pages::escape_html(&row.kind),
            pages::escape_html(&row.url),
            pages::escape_html(&row.title),
            pages::escape_html(&row.body),
            pages::escape_html(&row.created_at),
        ).expect("notification markup");
        if !row.is_read {
            write!(
                cards,
                "<form method=\"post\" action=\"/notifications/{}/read\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">Отметить прочитанным</button></form>",
                row.id, csrf_token,
            ).expect("read form");
        }
        cards.push_str("</article>");
    }
    if rows.is_empty() {
        let empty = if tab == "releases" {
            "Пока нет уведомлений о релизах."
        } else if tab == "system" {
            "Системных уведомлений пока нет."
        } else {
            "Уведомлений пока нет."
        };
        write!(cards, "<p>{empty}</p>").expect("notification empty state");
    }
    let older = if rows.len() == 50 {
        rows.last().map_or_else(String::new, |row| {
            format!(
                "<a href=\"/notifications?tab={tab}&amp;before={}\">Следующие уведомления</a>",
                row.id
            )
        })
    } else {
        String::new()
    };
    let body = format!(
        "<section class=\"me-page\"><div class=\"container\"><nav class=\"dashboard-nav\"><a class=\"dashboard-nav__item\" href=\"/me\">Обзор</a><a class=\"dashboard-nav__item\" href=\"/mail\">Личная почта</a><a class=\"dashboard-nav__item is-active\" href=\"/notifications\">Уведомления</a></nav><header class=\"ui-page-header\"><h1>Уведомления</h1></header><nav class=\"ui-tabs notifications-tabs\" aria-label=\"Раздел уведомлений\">{tab_links}</nav><form method=\"post\" action=\"/notifications/read-all\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf_token}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">Отметить все прочитанными</button></form><section class=\"me-section\" aria-label=\"Уведомления\">{cards}{older}</section></div></section>"
    );
    private_response(
        pages::layout("Уведомления", "Ваши уведомления", &body),
        &cookies,
    )
}

pub(super) async fn mark_one(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ReadForm>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match PgNotifications::new(state.accounts.pool().clone())
        .mark_read(actor, id)
        .await
    {
        Ok(true) => private_response(Redirect::to("/notifications"), &[]),
        Ok(false) => deny(StatusCode::NOT_FOUND),
        Err(e) => deny(super::mail::status(e)),
    }
}

pub(super) async fn mark_all(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ReadForm>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match PgNotifications::new(state.accounts.pool().clone())
        .mark_all_read(actor)
        .await
    {
        Ok(_) => private_response(Redirect::to("/notifications"), &[]),
        Err(e) => deny(super::mail::status(e)),
    }
}
