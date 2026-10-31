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
use std::{collections::HashMap, fmt::Write};
use tor_social::repositories::notifications::PgNotifications;

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/notifications", get(page))
        .route("/notifications/{id}/read", post(mark_one))
        .route("/notifications/read-all", post(mark_all))
        .route("/notifications/preferences", post(save_preferences))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NotificationQuery {
    before: Option<i64>,
    tab: Option<String>,
    notice: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadForm {
    csrf_token: String,
    next: Option<String>,
}

fn csrf(state: &AuthState, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Redirect only to a local page. The form's `next` is not an authority:
/// it is advisory, never a user identity or an ACL bypass.
fn safe_next_url(raw: Option<&str>) -> &str {
    let Some(raw) = raw else {
        return "/notifications";
    };
    let next = raw.trim();
    if next.is_empty()
        || next.len() > 1024
        || !next.starts_with('/')
        || next.starts_with("//")
        || next.contains('\\')
        || !next.is_ascii()
        || next.bytes().any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return "/notifications";
    }
    let path = next.split(['?', '#']).next().unwrap_or("");
    if path == "/api"
        || path.starts_with("/api/")
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return "/notifications";
    }
    next
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
    let requested_tab = query
        .tab
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
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
    let preferences = match store.preferences(actor).await {
        Ok(preferences) => preferences,
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
        let current = if value == tab {
            " aria-current=\"page\""
        } else {
            ""
        };
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
            "<article class=\"me-now-card\" data-notification-id=\"{}\" data-notification-kind=\"{}\"><strong>{}</strong><p>{}</p><time>{}</time><p>{status}</p>",
            row.id,
            pages::escape_html(&row.kind),
            pages::escape_html(&row.title),
            pages::escape_html(&row.body),
            pages::escape_html(&row.created_at),
        ).expect("notification markup");
        // The source 'Open' action acknowledges in POST before navigating.
        // Render it for read and unread rows; acknowledgement is idempotent.
        write!(
            cards,
            "<form method=\"post\" action=\"/notifications/{}/read\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"next\" value=\"{}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">Открыть</button></form>",
            row.id,
            csrf_token,
            pages::escape_html(&row.url),
        )
        .expect("notification open form");
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
    let mut preference_controls = String::new();
    for preference in &preferences {
        let checked = if preference.enabled { " checked" } else { "" };
        write!(
            preference_controls,
            r#"<label class="notifications-switch"><span class="notifications-switch__text"><strong>{}</strong><small>{}</small></span><input type="checkbox" name="pref_{}" value="on"{checked}></label>"#,
            pages::escape_html(preference.label),
            pages::escape_html(preference.description),
            preference.kind,
        )
        .expect("notification preference control");
    }
    let settings_notice = if query.notice.as_deref() == Some("preferences_saved") {
        r#"<p role="status">Настройки уведомлений сохранены.</p>"#
    } else {
        ""
    };
    let preferences_section = format!(
        r#"<aside class="notifications-settings-card" aria-labelledby="notifications-settings-title"><h2 id="notifications-settings-title">Настройки</h2><p>Выберите, какие события будут попадать в колокольчик.</p>{settings_notice}<form method="post" action="/notifications/preferences"><input type="hidden" name="csrf_token" value="{csrf_token}">{preference_controls}<button class="ui-button ui-button--primary" type="submit">Сохранить</button></form></aside>"#
    );
    let body = format!(
        "<section class=\"me-page\"><div class=\"container\"><nav class=\"dashboard-nav\"><a class=\"dashboard-nav__item\" href=\"/me\">Обзор</a><a class=\"dashboard-nav__item\" href=\"/mail\">Личная почта</a><a class=\"dashboard-nav__item is-active\" href=\"/notifications\">Уведомления</a></nav><header class=\"ui-page-header\"><h1>Уведомления</h1></header><nav class=\"ui-tabs notifications-tabs\" aria-label=\"Раздел уведомлений\">{tab_links}</nav><form method=\"post\" action=\"/notifications/read-all\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf_token}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">Отметить все прочитанными</button></form><section class=\"me-section\" aria-label=\"Уведомления\">{cards}{older}</section>{preferences_section}</div></section>"
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
    let next = safe_next_url(form.next.as_deref());
    match PgNotifications::new(state.accounts.pool().clone())
        .mark_read(actor, id)
        .await
    {
        Ok(true) => private_response(Redirect::to(next), &[]),
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

pub(super) async fn save_preferences(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(
        &state,
        &headers,
        fields.get("csrf_token").map(String::as_str).unwrap_or(""),
    ) {
        return deny(status);
    }
    // Unique pref_* names avoid repeated-key form-map ambiguity.
    // Unknown kinds and user IDs from the body are not authoritative.
    let enabled_kinds: Vec<String> = fields
        .iter()
        .filter_map(|(key, value)| {
            if value == "on" {
                key.strip_prefix("pref_").map(str::to_owned)
            } else {
                None
            }
        })
        .collect();
    match PgNotifications::new(state.accounts.pool().clone())
        .set_preferences(actor, &enabled_kinds)
        .await
    {
        Ok(()) => private_response(Redirect::to("/notifications?notice=preferences_saved"), &[]),
        Err(e) => deny(super::mail::status(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::safe_next_url;

    #[test]
    fn e08_source_notification_next_is_local_and_bounded() {
        for input in [
            None,
            Some(""),
            Some("https://attacker.invalid/steal"),
            Some("//attacker.invalid"),
            Some("/\\attacker.invalid"),
            Some("/api"),
            Some("/api/v1/me"),
            Some("/api?filter=1"),
            Some("/../mail/1"),
            Some("/mail/./1"),
            Some("/mail/1\\other"),
            Some("/mail/1\r\nLocation: https://attacker.invalid"),
            Some("/mail/1 raw"),
        ] {
            assert_eq!(safe_next_url(input), "/notifications", "{input:?}");
        }
        for valid in [
            "/mail/123",
            "/users/Mail%20reader%201",
            "/books/123#book-comments",
            "/notifications?tab=system",
        ] {
            assert_eq!(safe_next_url(Some(valid)), valid);
        }
        let too_long = format!("/{}", "x".repeat(1025));
        assert_eq!(safe_next_url(Some(&too_long)), "/notifications");
    }
}
