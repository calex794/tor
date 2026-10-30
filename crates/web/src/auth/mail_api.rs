//! E-08 JSON facade: same mail service as HTML; no body actor or command key.
use super::mail::{service, status};
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    session_user(state, headers)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)
}
fn csrf(state: &AuthState, headers: &HeaderMap) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|t| state.cookies.valid_csrf(headers, t))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueryMessages {
    conversation_id: Option<i64>,
    after_id: Option<i64>,
    before_id: Option<i64>,
    limit: Option<u16>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SendMessage {
    peer_id: String,
    text: String,
}
#[derive(Serialize)]
struct Message {
    id: String,
    sender_id: String,
    text: String,
    created_at: String,
}
#[derive(Serialize)]
struct MessagePage {
    messages: Vec<Message>,
    has_more: bool,
    newest_message_id: Option<String>,
    oldest_message_id: Option<String>,
    blocked: bool,
}
#[derive(Serialize)]
struct Conversation {
    id: String,
    peer_id: String,
    unread: i64,
    blocked: bool,
}
#[derive(Serialize)]
struct Mailbox {
    conversations: Vec<Conversation>,
    next_cursor: Option<String>,
}
#[derive(Serialize)]
struct Receipt {
    message_id: String,
    replayed: bool,
}

pub(super) async fn get(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<QueryMessages>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return deny(s),
    };
    let limit = query.limit.unwrap_or(50);
    if let Some(conversation) = query.conversation_id {
        let page = match service(&state)
            .messages(actor, conversation, query.after_id, query.before_id, limit)
            .await
        {
            Ok(p) => p,
            Err(e) => return deny(status(e)),
        };
        let result = MessagePage {
            newest_message_id: page.items.last().map(|m| m.id.to_string()),
            oldest_message_id: page.items.first().map(|m| m.id.to_string()),
            has_more: page.has_more,
            blocked: page.blocked,
            messages: page
                .items
                .into_iter()
                .map(|m| Message {
                    id: m.id.to_string(),
                    sender_id: m.sender_id.to_string(),
                    text: m.body,
                    created_at: m.created_at,
                })
                .collect(),
        };
        private_response(Json(result).into_response(), &[])
    } else {
        if query.before_id.is_some() {
            return deny(StatusCode::BAD_REQUEST);
        }
        let rows = match service(&state).mailbox(actor, query.after_id, limit).await {
            Ok(p) => p,
            Err(e) => return deny(status(e)),
        };
        let next_cursor = if rows.len() == usize::from(limit) {
            rows.last().map(|c| c.id.to_string())
        } else {
            None
        };
        let result = Mailbox {
            conversations: rows
                .into_iter()
                .map(|c| Conversation {
                    id: c.id.to_string(),
                    peer_id: c.peer_id.to_string(),
                    unread: c.unread,
                    blocked: c.blocked,
                })
                .collect(),
            next_cursor,
        };
        private_response(Json(result).into_response(), &[])
    }
}

pub(super) async fn post(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<SendMessage>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(a) => a,
        Err(s) => return deny(s),
    };
    if let Err(s) = csrf(&state, &headers) {
        return deny(s);
    }
    let key = match headers.get("idempotency-key").and_then(|h| h.to_str().ok()) {
        Some(k) => k,
        None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let peer = match input.peer_id.parse::<i64>() {
        Ok(id) if id > 0 && input.peer_id == id.to_string() => id,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    match service(&state)
        .send_to_peer(actor, peer, &input.text, key)
        .await
    {
        Ok(receipt) => private_response(
            Json(Receipt {
                message_id: receipt.id.to_string(),
                replayed: receipt.replayed,
            })
            .into_response(),
            &[],
        ),
        Err(e) => deny(status(e)),
    }
}
