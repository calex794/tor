//! MOD-OPS-ALERT-01: private actionable staff inbox for observed overdue
//! requests; read-only, no individual recipient delivery or push/mail.
use super::{deny, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_accounts::{
    repositories::{
        moderation_books::ModerationBooksError,
        moderation_overdue_inbox::OverdueInboxItem,
    },
    services::security::session_token_digest,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InboxQuery {
    before: Option<i64>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/moderation/books/alerts", get(page))
}

fn target(kind: &str, threshold: i16) -> (&'static str, &'static str) {
    let old = threshold == 72;
    match (kind, old) {
        ("registration", true) => (
            "Регистрация книги",
            "/moderation/books/registrations?wait=gte72",
        ),
        ("registration", false) => (
            "Регистрация книги",
            "/moderation/books/registrations?wait=gte24",
        ),
        ("cover", true) => ("Обложка", "/moderation/covers?wait=gte72"),
        ("cover", false) => ("Обложка", "/moderation/covers?wait=gte24"),
        ("deletion", true) => ("Удаление", "/moderation/books/deletions?wait=gte72"),
        ("deletion", false) => ("Удаление", "/moderation/books/deletions?wait=gte24"),
        _ => ("Неизвестная заявка", "/moderation/books/registrations"),
    }
}

fn render(rows: &[OverdueInboxItem]) -> String {
    let mut items = String::new();
    for r in rows {
        let (label, href) = target(&r.request_kind, r.threshold_hours);
        let id = r.source_id;
        let revision = r.source_revision;
        let hours = r.threshold_hours;
        let utc = pages::escape_html(&r.first_seen_utc);
        items.push_str(&format!(
            r#"<li><strong>{label} #{id}</strong>
                <span>ревизия {revision}, ожидание от {hours} ч</span>
                <span>замечено {utc} UTC</span>
                <a href="{href}">Открыть очередь</a></li>"#
        ));
    }
    if rows.is_empty() {
        items.push_str("<li>Нет действующих просроченных сигналов</li>");
    }
    let next = rows
        .last()
        .filter(|_| rows.len() == 40)
        .map(|item| {
            format!(
                r#"<p><a href="/moderation/books/alerts?before={}">Следующая страница</a></p>"#,
                item.id
            )
        })
        .unwrap_or_default();
    pages::layout(
        "Служебные сигналы модерации",
        &format!(
            r#"<main><h1>Служебные сигналы модерации</h1>
              <nav aria-label="Очереди модерации">
                <a href="/moderation/books/registrations">Регистрация</a>
                <a href="/moderation/covers">Обложки</a>
                <a href="/moderation/books/deletions">Удаление</a>
                <a href="/moderation/books/alerts" aria-current="page">Сигналы ожидания</a>
              </nav>
              <p>Это приватные наблюдения, не отправленные уведомления.
                 Приоритеты 24/72 ч ещё не утверждены как SLA. История
                 разрешённых заявок не показывается.</p>
              <ul>{items}</ul>{next}
            </main>"#
        ),
    )
    .0
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<InboxQuery>,
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
    let rows = match state.accounts.moderation_overdue_inbox(&digest, query.before).await {
        Ok(rows) => rows,
        Err(ModerationBooksError::Denied) => return deny(StatusCode::FORBIDDEN),
        Err(ModerationBooksError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(ModerationBooksError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        Err(ModerationBooksError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(ModerationBooksError::Stale) => return deny(StatusCode::CONFLICT),
    };
    let mut reply = private_response(Html(render(&rows)), &[]);
    reply.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    reply
}

#[cfg(test)]
mod tests {
    use super::{render, OverdueInboxItem};

    #[test]
    fn inbox_never_renders_source_content_or_arbitrary_links() {
        let html = render(&[
            OverdueInboxItem {
                id: 11,
                request_kind: "deletion".to_owned(),
                source_id: 24,
                source_revision: 2,
                threshold_hours: 72,
                first_seen_utc: "2026-11-07 10:22".to_owned(),
            },
            OverdueInboxItem {
                id: 12,
                request_kind: "cover".to_owned(),
                source_id: 7,
                source_revision: 0,
                threshold_hours: 24,
                first_seen_utc: "<script>bad</script>".to_owned(),
            },
        ]);
        assert!(html.contains("/moderation/books/deletions?wait=gte72"));
        assert!(html.contains("/moderation/covers?wait=gte24"));
        assert!(html.contains("&lt;script&gt;bad&lt;/script&gt;"));
        assert!(!html.contains("<script>bad</script>"));
    }
}
