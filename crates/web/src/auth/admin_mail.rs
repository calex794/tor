//! Read-only audited administrator views of direct user mail (E-08/S057).
//! Never accept a claimed administrator ID or read private data without audit.
use super::{deny, session_user, AuthState};
use crate::{
    auth_cookies::private_response,
    pages::{escape_html, layout},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_social::repositories::admin_mail::{AdminDirectMessage, PgAdminMail};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Search {
    q: Option<String>,
    before: Option<i64>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    before: Option<i64>,
}

// PgAdminMail rechecks active session, account and administrator role
// under row locks, then commits the immutable access audit before returning.
async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
    session_user(state, headers)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(session_token_digest(&token))
}

fn shell(title: &str, body: &str) -> Html<String> {
    layout(
        title,
        "Административное чтение личной почты с аудитом.",
        &format!(
            r#"<section class="me-page mail-page"><div class="container"><nav aria-label="Администрирование"><a href="/admin/mail">Личная почта</a> · <a href="/admin/mail/groups">Группы</a> · <a href="/admin/mail/book-rooms">Книжные чаты</a></nav>{body}</div></section>"#
        ),
    )
}

fn encode_query(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            write!(out, "%{byte:02X}").expect("url fragment");
        }
    }
    out
}

pub(super) async fn index(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<Search>,
) -> Response {
    let digest = match actor(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let q = query.q.as_deref().unwrap_or("").trim();
    let users = match PgAdminMail::new(state.accounts.pool().clone())
        .users(&digest, q, query.before, 50)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let mut body = format!(
        r#"<header class="ui-page-header"><h1>Почта пользователей</h1></header><section class="me-section"><p>Все просмотры регистрируются в журнале аудита.</p><form action="/admin/mail" method="get"><label for="mail-search">Имя пользователя</label><input class="ui-input" id="mail-search" name="q" maxlength="80" value="{}"><button class="ui-button" type="submit">Поиск</button></form>"#,
        escape_html(q)
    );
    if users.is_empty() {
        body.push_str("<p>Пользователи не найдены.</p>");
    }
    for user in &users {
        write!(body, r#"<article class="me-now-card"><a href="/admin/mail/users/{}">{}</a><p>Диалогов: {}</p></article>"#, user.user_id, escape_html(&user.display_name), user.conversation_count).expect("user");
    }
    if users.len() == 50 {
        if let Some(last) = users.last() {
            write!(
                body,
                r#"<a href="/admin/mail?q={}&amp;before={}">Предыдущие пользователи</a>"#,
                encode_query(q),
                last.user_id
            )
            .expect("pagination");
        }
    }
    body.push_str("</section>");
    private_response(shell("Почта пользователей", &body), &[])
}

pub(super) async fn user(
    State(state): State<AuthState>,
    Path(user_id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Response {
    let digest = match actor(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let rows = match PgAdminMail::new(state.accounts.pool().clone())
        .mailbox(&digest, user_id, query.before, 50)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let mut body =
        format!(r#"<h1>Диалоги пользователя #{user_id}</h1><section class="me-section">"#);
    if rows.is_empty() {
        body.push_str("<p>Переписок нет.</p>");
    }
    for row in &rows {
        write!(body, r#"<article class="me-now-card"><a href="/admin/mail/{}">Диалог #{}</a><p>Участники: {} и {}. Сообщений: {}</p></article>"#, row.id, row.id, row.user_low, row.user_high, row.message_count).expect("mailbox");
    }
    if rows.len() == 50 {
        if let Some(last) = rows.last() {
            write!(
                body,
                r#"<a href="/admin/mail/users/{user_id}?before={}">Предыдущие диалоги</a>"#,
                last.id
            )
            .expect("pagination");
        }
    }
    body.push_str("</section>");
    private_response(shell("Диалоги пользователя", &body), &[])
}

pub(super) async fn conversation(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Response {
    let digest = match actor(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let rows = match PgAdminMail::new(state.accounts.pool().clone())
        .messages(&digest, id, query.before, 50)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let mut body =
        format!(r#"<h1>Переписка #{id}</h1><section class="me-section" aria-label="Сообщения">"#);
    if rows.is_empty() {
        body.push_str("<p>Сообщений нет.</p>");
    }
    for row in &rows {
        write!(body, r#"<article class="me-now-card" data-message-id="{}"><strong>{}</strong><p class="mail-message-body">{}</p></article>"#, row.id, escape_html(&row.sender_name), escape_html(&row.body)).expect("message");
    }
    if rows.len() == 50 {
        if let Some(first) = rows.first() {
            write!(
                body,
                r#"<a href="/admin/mail/{id}?before={}">Предыдущие сообщения</a>"#,
                first.id
            )
            .expect("pagination");
        }
    }
    body.push_str("</section>");
    private_response(shell("Переписка пользователей", &body), &[])
}

fn messages_section(rows: &[AdminDirectMessage], older_link: &str) -> String {
    let mut body = String::from(r#"<section class="me-section" aria-label="Сообщения">"#);
    if rows.is_empty() {
        body.push_str("<p>Сообщений нет.</p>");
    }
    for row in rows {
        write!(
            body,
            r#"<article class="me-now-card" data-message-id="{}"><strong>{}</strong><p class="mail-message-body">{}</p></article>"#,
            row.id,
            escape_html(&row.sender_name),
            escape_html(&row.body),
        )
        .expect("admin chat message");
    }
    if rows.len() == 50 {
        if let Some(first) = rows.first() {
            write!(
                body,
                r#"<a href="{older_link}?before={}">Предыдущие сообщения</a>"#,
                first.id,
            )
            .expect("admin history cursor");
        }
    }
    body.push_str("</section>");
    body
}

pub(super) async fn groups(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Response {
    let digest = match actor(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let rows = match PgAdminMail::new(state.accounts.pool().clone())
        .groups(&digest, query.before, 50)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let mut body = String::from(r#"<h1>Групповые чаты</h1><section class="me-section">"#);
    if rows.is_empty() {
        body.push_str("<p>Групп пока нет.</p>");
    }
    for group in &rows {
        write!(
            body,
            r#"<article class="me-now-card"><a href="/admin/mail/groups/{}">{}</a><p>Сообщений: {}</p></article>"#,
            group.id,
            escape_html(&group.title),
            group.message_count,
        )
        .expect("group list");
    }
    if rows.len() == 50 {
        if let Some(last) = rows.last() {
            write!(
                body,
                r#"<a href="/admin/mail/groups?before={}">Предыдущие группы</a>"#,
                last.id,
            )
            .expect("group pagination");
        }
    }
    body.push_str("</section>");
    private_response(shell("Групповые чаты", &body), &[])
}

pub(super) async fn group(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Response {
    let digest = match actor(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let rows = match PgAdminMail::new(state.accounts.pool().clone())
        .group(&digest, id, query.before, 50)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let body = format!(
        "<h1>Групповой чат #{id}</h1>{}",
        messages_section(&rows, &format!("/admin/mail/groups/{id}")),
    );
    private_response(shell("Сообщения группы", &body), &[])
}

pub(super) async fn book_rooms(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Response {
    let digest = match actor(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let rows = match PgAdminMail::new(state.accounts.pool().clone())
        .book_rooms(&digest, query.before, 50)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let mut body = String::from(r#"<h1>Книжные чаты</h1><section class="me-section">"#);
    if rows.is_empty() {
        body.push_str("<p>Книжных чатов пока нет.</p>");
    }
    for book in &rows {
        write!(
            body,
            r#"<article class="me-now-card"><a href="/admin/mail/book-rooms/{}">{}</a><p>Сообщений: {}</p></article>"#,
            book.book_id,
            escape_html(&book.title),
            book.message_count,
        )
        .expect("book-room list");
    }
    if rows.len() == 50 {
        if let Some(last) = rows.last() {
            write!(
                body,
                r#"<a href="/admin/mail/book-rooms?before={}">Предыдущие книжные чаты</a>"#,
                last.book_id,
            )
            .expect("book-room pagination");
        }
    }
    body.push_str("</section>");
    private_response(shell("Книжные чаты", &body), &[])
}

pub(super) async fn book_room(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Response {
    let digest = match actor(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let rows = match PgAdminMail::new(state.accounts.pool().clone())
        .book_room(&digest, id, query.before, 50)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let body = format!(
        "<h1>Книжный чат #{id}</h1>{}",
        messages_section(&rows, &format!("/admin/mail/book-rooms/{id}")),
    );
    private_response(shell("Сообщения книжного чата", &body), &[])
}

#[cfg(test)]
mod tests {
    use super::encode_query;

    #[test]
    fn e08_admin_mail_encodes_search_query() {
        assert_eq!(encode_query("a & <"), "a%20%26%20%3C");
        assert_eq!(encode_query("А"), "%D0%90");
    }
}
