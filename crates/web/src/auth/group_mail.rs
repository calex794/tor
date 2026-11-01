//! E-08/S057 group mail browser pages, separate from direct mail and support.
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
use tor_social::repositories::group_mail::{GroupConversation, GroupMessage, PgGroupMail};

fn group_store(state: &AuthState) -> PgGroupMail {
    PgGroupMail::new(state.accounts.pool().clone())
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

fn shell(body: &str) -> Html<String> {
    layout(
        "Групповая почта",
        "Личные групповые переписки.",
        &format!(
            r#"<section class="me-page mail-page"><div class="container"><nav class="dashboard-nav" aria-label="Разделы кабинета"><a class="dashboard-nav__item" href="/me">Обзор</a><a class="dashboard-nav__item" href="/mail">Личная почта</a><a class="dashboard-nav__item is-active" href="/mail/groups">Группы</a><a class="dashboard-nav__item" href="/notifications">Уведомления</a></nav>{body}</div></section>"#
        ),
    )
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
pub(super) struct GroupsQuery {
    before: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateForm {
    title: String,
    peer_one: i64,
    peer_two: Option<String>,
    csrf_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SendForm {
    body: String,
    command_key: String,
    csrf_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LeaveForm {
    csrf_token: String,
}

fn render_index(
    rows: &[GroupConversation],
    contacts: &[(i64, String)],
    csrf: &str,
) -> Html<String> {
    let mut body = String::from(
        r#"<header class="ui-page-header"><h1 class="ui-page-header__title">Группы</h1></header><section class="me-section" aria-label="Групповые переписки">"#,
    );
    if rows.is_empty() {
        body.push_str("<p>Групповых переписок пока нет.</p>");
    }
    for row in rows {
        write!(
            body,
            r#"<article class="me-now-card"><a href="/mail/groups/{}">{}</a></article>"#,
            row.id,
            escape_html(&row.title)
        )
        .expect("group link");
    }
    if rows.len() == 50 {
        if let Some(last) = rows.last() {
            write!(
                body,
                r#"<a href="/mail/groups?before={}">Предыдущие группы</a>"#,
                last.id
            )
            .expect("group cursor");
        }
    }
    body.push_str("</section><section class=\"me-section\"><h2>Создать группу</h2>");
    if contacts.is_empty() {
        body.push_str(
            "<p>Для создания группы нужен хотя бы один собеседник из личной почты. Начните переписку через профиль пользователя.</p>",
        );
    } else {
        write!(
            body,
            r#"<form method="post" action="/mail/groups" class="comments-form"><label for="group-title">Название</label><input id="group-title" class="ui-input" name="title" maxlength="120" required><input type="hidden" name="csrf_token" value="{}">"#,
            escape_html(csrf)
        )
        .expect("group create form");
        for (field, caption, required) in [
            ("peer_one", "Первый участник", "required"),
            ("peer_two", "Второй участник (необязательно)", ""),
        ] {
            write!(
                body,
                r#"<label for="{field}">{caption}</label><select id="{field}" name="{field}" {required}><option value="">Выберите собеседника</option>"#
            )
            .expect("group peer selector");
            for (id, name) in contacts {
                write!(
                    body,
                    r#"<option value="{id}">{}</option>"#,
                    escape_html(name)
                )
                .expect("group peer option");
            }
            body.push_str("</select>");
        }
        body.push_str(r#"<button class="ui-button ui-button--primary" type="submit">Создать группу</button></form>"#);
    }
    body.push_str("</section>");
    shell(&body)
}

fn render_detail(
    group: &GroupConversation,
    messages: &[GroupMessage],
    members: &[(i64, String)],
    csrf: &str,
    command_key: &str,
    actor: i64,
) -> Html<String> {
    let id = group.id;
    let mut body = format!(
        r#"<header class="ui-page-header"><h1 class="ui-page-header__title">{}</h1><p><a href="/mail/groups">Все группы</a></p></header><section class="me-section" aria-label="Сообщения группы">"#,
        escape_html(&group.title)
    );
    write!(
        body,
        r#"<div class="group-participants"><h2>Участники ({})</h2><ul>"#,
        members.len()
    )
    .expect("group member count");
    for (member_id, name) in members {
        let suffix = if *member_id == actor { " (вы)" } else { "" };
        write!(
            body,
            r#"<li data-member-id="{member_id}">{}{suffix}</li>"#,
            escape_html(name)
        )
        .expect("group member");
    }
    body.push_str("</ul></div>");
    for message in messages {
        let sender: &str = if message.sender_id == actor {
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
        .expect("group message");
    }
    if messages.len() == 50 {
        if let Some(first) = messages.first() {
            write!(
                body,
                r#"<a href="/mail/groups/{id}?before={}">Предыдущие сообщения</a>"#,
                first.id
            )
            .expect("group message cursor");
        }
    }
    let csrf = escape_html(csrf);
    write!(
        body,
        r#"</section><section class="me-section"><form class="comments-form" method="post" action="/mail/groups/{id}/messages"><label for="group-message-body">Сообщение</label><textarea id="group-message-body" name="body" class="ui-textarea" rows="3" required></textarea><input type="hidden" name="command_key" value="{}"><input type="hidden" name="csrf_token" value="{csrf}"><button class="ui-button ui-button--primary" type="submit">Отправить сообщение</button></form>"#,
        escape_html(command_key)
    )
    .expect("group send form");
    if group.is_creator {
        body.push_str("<p>Создатель группы пока не может выйти из неё.</p>");
    } else {
        write!(
            body,
            r#"<form method="post" action="/mail/groups/{id}/leave"><input type="hidden" name="csrf_token" value="{csrf}"><button class="ui-button ui-button--ghost" type="submit">Выйти из группы</button></form>"#
        )
        .expect("leave form");
    }
    body.push_str("</section>");
    shell(&body)
}

pub(super) async fn index(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<GroupsQuery>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let rows = match group_store(&state).list(actor, query.before, 50).await {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let conversations = match super::mail::service(&state).mailbox(actor, None, 100).await {
        Ok(rows) => rows,
        Err(error) => return deny(super::mail::status(error)),
    };
    let mut contacts = Vec::with_capacity(conversations.len());
    for conversation in conversations {
        match state.accounts.user_display_name(conversation.peer_id).await {
            Ok(Some(name)) => contacts.push((conversation.peer_id, name)),
            Ok(None) => {}
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    }
    let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(render_index(&rows, &contacts, &csrf_token), &cookies);
    secure(&mut response);
    response
}

pub(super) async fn create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    if form.peer_one <= 0 {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut members = vec![form.peer_one];
    if let Some(raw) = form.peer_two.as_deref() {
        let raw = raw.trim();
        if !raw.is_empty() {
            match raw.parse::<i64>() {
                Ok(peer) if peer > 0 && peer != form.peer_one => members.push(peer),
                _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
            }
        }
    }
    match group_store(&state)
        .create(actor, &members, &form.title)
        .await
    {
        Ok(id) => private_response(Redirect::to(&format!("/mail/groups/{id}")), &[]),
        Err(error) => deny(super::mail::status(error)),
    }
}

pub(super) async fn detail(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<GroupsQuery>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let store = group_store(&state);
    let group = match store.get(actor, id).await {
        Ok(group) => group,
        Err(error) => return deny(super::mail::status(error)),
    };
    let messages = match store.messages(actor, id, query.before, 50).await {
        Ok(messages) => messages,
        Err(error) => return deny(super::mail::status(error)),
    };
    let members = match store.members(actor, id).await {
        Ok(members) => members,
        Err(error) => return deny(super::mail::status(error)),
    };
    let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        render_detail(
            &group,
            &messages,
            &members,
            &csrf_token,
            &generate_session_token(),
            actor,
        ),
        &cookies,
    );
    secure(&mut response);
    response
}

pub(super) async fn send(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<SendForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state)
        .send(actor, id, &form.body, &form.command_key)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/mail/groups/{id}")), &[]),
        Err(error) => deny(super::mail::status(error)),
    }
}

pub(super) async fn leave(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<LeaveForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state).leave(actor, id).await {
        Ok(()) => private_response(Redirect::to("/mail/groups"), &[]),
        Err(error) => deny(super::mail::status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_08_group_pages_escape_untrusted_titles_and_messages() {
        let group = GroupConversation {
            id: 5,
            title: "<script>unsafe</script>".to_owned(),
            is_creator: false,
        };
        let messages = vec![GroupMessage {
            id: 7,
            sender_id: 2,
            body: "<img src=x onerror=alert(1)>".to_owned(),
            sender_name: "<iframe src=evil>".to_owned(),
        }];
        let members = vec![(1, "<svg onload=alert(1)>".to_owned())];
        let html = render_detail(&group, &messages, &members, "<csrf>", &"x".repeat(43), 1).0;
        assert!(html.contains("&lt;script&gt;unsafe&lt;/script&gt;"));
        assert!(html.contains("Участники (1)"));
        assert!(html.contains("&lt;svg onload=alert(1)&gt;"));
        assert!(!html.contains("<svg onload=alert(1)>"));
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(html.contains("&lt;iframe src=evil&gt;"));
        assert!(!html.contains("<iframe src=evil>"));
        assert!(!html.contains("<script>unsafe</script>"));
        assert!(!html.contains("<img src=x onerror=alert(1)>"));
        assert!(html.contains("name=\"csrf_token\" value=\"&lt;csrf&gt;\""));
    }
}
