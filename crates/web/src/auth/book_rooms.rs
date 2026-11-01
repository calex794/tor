//! E-08/S057: server-rendered public book room with explicit membership.
use super::{deny, session_user, AuthState};
use crate::{
    auth_cookies::private_response,
    pages::{escape_html, layout},
};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_social::repositories::book_rooms::{BookRoomMessage, PgBookRooms};

fn store(state: &AuthState) -> PgBookRooms {
    PgBookRooms::new(state.accounts.pool().clone())
}

async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    session_user(state, headers)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn csrf(state: &AuthState, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

fn secure(response: &mut Response) {
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; script-src 'none'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RoomQuery {
    before: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TokenForm {
    csrf_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageForm {
    body: String,
    command_key: String,
    csrf_token: String,
}

fn render(
    book_id: i64,
    title: &str,
    joined: bool,
    messages: &[BookRoomMessage],
    csrf_token: &str,
    command_key: &str,
    actor: i64,
) -> Html<String> {
    let title = escape_html(title);
    let csrf_token = escape_html(csrf_token);
    let mut body = format!(
        r#"<section class="me-page mail-page"><div class="container"><nav class="breadcrumbs" aria-label="Навигация"><a href="/books/{book_id}">Книга</a> / <span>Чат</span></nav><h1>Книжный чат: {title}</h1>"#
    );
    if !joined {
        write!(
            body,
            r#"<p>Чтобы читать сообщения и общаться с читателями, вступите в книжный чат.</p><form method="post" action="/books/{book_id}/room/join"><input type="hidden" name="csrf_token" value="{csrf_token}"><button class="ui-button ui-button--primary" type="submit">Вступить в чат</button></form>"#
        )
        .expect("join form");
    } else {
        body.push_str(r#"<section class="me-section" aria-label="Сообщения книжного чата"><h2>Сообщения</h2>"#);
        if messages.is_empty() {
            body.push_str("<p>Пока сообщений нет.</p>");
        }
        for message in messages {
            let sender = if message.sender_id == actor {
                "Вы"
            } else {
                &message.sender_name
            };
            write!(
                body,
                r#"<article class="me-now-card" data-message-id="{}"><strong>{}</strong><p class="mail-message-body">{}</p></article>"#,
                message.id,
                escape_html(sender),
                escape_html(&message.body)
            )
            .expect("render message");
        }
        if messages.len() == 50 {
            if let Some(first) = messages.first() {
                write!(
                    body,
                    r#"<nav aria-label="Страницы переписки"><a href="/books/{book_id}/room?before={}">Предыдущие сообщения</a></nav>"#,
                    first.id
                )
                .expect("page cursor");
            }
        }
        write!(
            body,
            r#"</section><section class="me-section"><h2>Написать сообщение</h2><form class="comments-form" method="post" action="/books/{book_id}/room/messages"><label for="room-body">Ваше сообщение</label><textarea id="room-body" name="body" class="ui-textarea" rows="3" maxlength="4000" required></textarea><input type="hidden" name="command_key" value="{}"><input type="hidden" name="csrf_token" value="{csrf_token}"><button class="ui-button ui-button--primary" type="submit">Отправить</button></form><form method="post" action="/books/{book_id}/room/leave"><input type="hidden" name="csrf_token" value="{csrf_token}"><button class="ui-button ui-button--ghost" type="submit">Выйти из чата</button></form></section>"#,
            escape_html(command_key)
        )
        .expect("send and leave forms");
    }
    body.push_str("</div></section>");
    layout("Книжный чат", "Сообщения читателей книги.", &body)
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<RoomQuery>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let rooms = store(&state);
    let (title, joined) = match rooms.overview(actor, id).await {
        Ok(details) => details,
        Err(error) => return deny(super::mail::status(error)),
    };
    let messages = if joined {
        match rooms.messages(actor, id, query.before, 50).await {
            Ok(messages) => messages,
            Err(error) => return deny(super::mail::status(error)),
        }
    } else {
        if query.before.is_some() {
            return deny(StatusCode::NOT_FOUND);
        }
        Vec::new()
    };
    let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        render(
            id,
            &title,
            joined,
            &messages,
            &csrf_token,
            &generate_session_token(),
            actor,
        ),
        &cookies,
    );
    secure(&mut response);
    response
}

pub(super) async fn join(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match store(&state).join(actor, id).await {
        Ok(()) => private_response(Redirect::to(&format!("/books/{id}/room")), &[]),
        Err(error) => deny(super::mail::status(error)),
    }
}

pub(super) async fn send(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<MessageForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match store(&state)
        .send(actor, id, &form.body, &form.command_key)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/books/{id}/room")), &[]),
        Err(error) => deny(super::mail::status(error)),
    }
}

pub(super) async fn leave(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match store(&state).leave(actor, id).await {
        Ok(()) => private_response(Redirect::to(&format!("/books/{id}/room")), &[]),
        Err(error) => deny(super::mail::status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_08_room_escapes_untrusted_text_and_only_renders_member_forms() {
        let messages = vec![BookRoomMessage {
            id: 5,
            sender_id: 22,
            sender_name: "<svg onload=alert(1)>".to_owned(),
            body: "<img src=x onerror=alert(2)>".to_owned(),
        }];
        let html = render(3, "<script>alert(0)</script>", true, &messages, "<csrf>", &"x".repeat(43), 11).0;
        assert!(html.contains("&lt;script&gt;alert(0)&lt;/script&gt;"));
        assert!(html.contains("&lt;svg onload=alert(1)&gt;"));
        assert!(html.contains("&lt;img src=x onerror=alert(2)&gt;"));
        assert!(!html.contains("<script>alert(0)</script>"));
        assert!(!html.contains("<svg onload=alert(1)>"));
        assert!(!html.contains("<img src=x onerror=alert(2)>"));
        assert!(html.contains("name=\"csrf_token\" value=\"&lt;csrf&gt;\""));
        let guest = render(3, "Public book", false, &messages, "csrf", "key", 11).0;
        assert!(guest.contains("Вступить в чат"));
        assert!(!guest.contains("Отправить</button>"));
        assert!(!guest.contains("onerror=alert(2)"));
    }
}
