//! Browser workflow for explicit group invitations. No invitation is membership.
use super::super::{deny, session_user, AuthState};
use super::{group_store, secure, shell};
use crate::{auth_cookies::private_response, pages::escape_html};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;

pub(crate) fn router() -> Router<AuthState> {
    Router::new()
        .route("/mail/group-invitations", get(inbox))
        .route("/mail/groups/{id}/invite", get(invite_page).post(invite))
        .route("/mail/group-invitations/{id}/accept", post(accept))
        .route("/mail/group-invitations/{id}/decline", post(decline))
        .route(
            "/mail/groups/{id}/invite/{invitation_id}/revoke",
            post(revoke),
        )
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteForm {
    invited_user_id: i64,
    csrf_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionForm {
    csrf_token: String,
}

async fn inbox(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let invites = match group_store(&state).pending_for(actor).await {
        Ok(invites) => invites,
        Err(error) => return deny(super::super::mail::status(error)),
    };
    let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
    let csrf_token = escape_html(&csrf_token);
    let mut body = String::from(
        "<header class=\"ui-page-header\"><h1>Приглашения в группы</h1><a href=\"/mail/groups\">К группам</a></header>",
    );
    if invites.is_empty() {
        body.push_str("<p>Новых приглашений нет.</p>");
    }
    for invite in &invites {
        write!(
            body,
            r#"<article class="me-now-card"><h2>{}</h2><p>Приглашает: {}</p><p>Доступ к сообщениям появится только после принятия.</p><form method="post" action="/mail/group-invitations/{}/accept"><input type="hidden" name="csrf_token" value="{}"><button type="submit">Принять</button></form><form method="post" action="/mail/group-invitations/{}/decline"><input type="hidden" name="csrf_token" value="{}"><button type="submit">Отклонить</button></form></article>"#,
            escape_html(&invite.title),
            escape_html(&invite.inviter_name),
            invite.id,
            csrf_token,
            invite.id,
            csrf_token,
        )
        .expect("invitation card");
    }
    let mut response = private_response(shell(&body), &cookies);
    secure(&mut response, false);
    response
}

async fn invite_page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let store = group_store(&state);
    let group = match store.get(actor, id).await {
        Ok(group) if group.is_creator => group,
        Ok(_) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(super::super::mail::status(error)),
    };
    let members = match store.members(actor, id).await {
        Ok(members) => members,
        Err(error) => return deny(super::super::mail::status(error)),
    };
    let sent = match store.pending_sent(actor, id).await {
        Ok(invites) => invites,
        Err(error) => return deny(super::super::mail::status(error)),
    };
    let conversations = match super::super::mail::service(&state)
        .mailbox(actor, None, 100)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(super::super::mail::status(error)),
    };
    let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
    let csrf_token = escape_html(&csrf_token);
    let mut body = format!(
        r#"<header class="ui-page-header"><h1>Пригласить в {}</h1><a href="/mail/groups/{id}">К переписке</a></header><p>Приглашение действует 7 дней. До подтверждения пользователь не получает доступ к сообщениям.</p><form method="post" action="/mail/groups/{id}/invite"><label for="group-invite-user">Получатель</label><select id="group-invite-user" name="invited_user_id" required><option value="">Выберите собеседника</option>"#,
        escape_html(&group.title),
    );
    let former = match store.former_members(actor, id).await {
        Ok(rows) => rows,
        Err(error) => return deny(super::super::mail::status(error)),
    };
    let mut choices = 0;
    let mut seen = std::collections::HashSet::new();
    for (member_id, name) in former {
        if seen.insert(member_id) {
            write!(
                body,
                r#"<option value="{}">{}</option>"#,
                member_id,
                escape_html(&name),
            )
            .expect("former member option");
            choices += 1;
        }
    }
    for conversation in conversations {
        if !seen.insert(conversation.peer_id) {
            continue;
        }
        if members
            .iter()
            .any(|(member_id, _)| *member_id == conversation.peer_id)
        {
            continue;
        }
        let name = match state.accounts.user_display_name(conversation.peer_id).await {
            Ok(Some(name)) => name,
            Ok(None) => continue,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        };
        write!(
            body,
            r#"<option value="{}">{}</option>"#,
            conversation.peer_id,
            escape_html(&name),
        )
        .expect("invite option");
        choices += 1;
    }
    write!(
        body,
        r#"</select><input type="hidden" name="csrf_token" value="{csrf_token}"><button type="submit">Отправить приглашение</button></form>"#,
    )
    .expect("invite form");
    if choices == 0 {
        body.push_str(
            "<p>Нет доступных собеседников. Начните личную переписку с нужным пользователем.</p>",
        );
    }
    if !sent.is_empty() {
        body.push_str("<h2>Ожидают ответа</h2>");
        for (invitation_id, name) in sent {
            write!(
                body,
                r#"<article class="me-now-card"><strong>{}</strong><form method="post" action="/mail/groups/{id}/invite/{invitation_id}/revoke"><input type="hidden" name="csrf_token" value="{csrf_token}"><button type="submit">Отозвать</button></form></article>"#,
                escape_html(&name),
            )
            .expect("revoke form");
        }
    }
    let mut response = private_response(shell(&body), &cookies);
    secure(&mut response, false);
    response
}

async fn invite(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<InviteForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state)
        .invite(actor, id, form.invited_user_id)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/mail/groups/{id}/invite")), &[]),
        Err(error) => deny(super::super::mail::status(error)),
    }
}

async fn accept(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state).accept_invite(actor, id).await {
        Ok(group) => private_response(Redirect::to(&format!("/mail/groups/{group}")), &[]),
        Err(error) => deny(super::super::mail::status(error)),
    }
}

async fn decline(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state).decline_invite(actor, id).await {
        Ok(()) => private_response(Redirect::to("/mail/group-invitations"), &[]),
        Err(error) => deny(super::super::mail::status(error)),
    }
}

async fn revoke(
    State(state): State<AuthState>,
    Path((id, invitation_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    match group_store(&state)
        .revoke_invite(actor, id, invitation_id)
        .await
    {
        Ok(()) => private_response(Redirect::to(&format!("/mail/groups/{id}/invite")), &[]),
        Err(error) => deny(super::super::mail::status(error)),
    }
}
