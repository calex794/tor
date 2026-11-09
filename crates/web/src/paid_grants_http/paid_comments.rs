//! S011 paid immutable-edition discussions. Only D-authorized buyers and
//! live A sessions may read or write E canonical comments; no paid plaintext.
use super::{deny, positive, GrantState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tor_accounts::{
    repositories::scoped_actions::ScopedActionAdmission, services::security::session_token_digest,
};
use tor_contracts::paid_delivery::{Actor, EntitlementDecision, EntitlementPort, PositiveId};
use tor_discussions::{
    domain::comments::{CommentBody, CommentIntentKey},
    repositories::{
        paid_chapter_comments::PgPaidChapterComments,
        public_chapter_comments::{CreateResult, ListError, NewAnchor, NewChapterComment},
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommentQuery {
    purchase_id: String,
    version_id: String,
    after: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommentInput {
    purchase_id: String,
    version_id: String,
    body: String,
    command_key: String,
    parent_id: Option<i64>,
    anchor_block: Option<i32>,
    anchor_start: Option<i32>,
    anchor_end: Option<i32>,
    anchor_quote: Option<String>,
}

fn parsed_anchor(body: &CommentInput) -> Result<Option<NewAnchor<'_>>, StatusCode> {
    match (
        body.anchor_block,
        body.anchor_start,
        body.anchor_end,
        body.anchor_quote.as_deref(),
    ) {
        (None, None, None, None) => Ok(None),
        (Some(block_index), Some(start_utf16), Some(end_utf16), Some(quote))
            if body.parent_id.is_none() =>
        {
            Ok(Some(NewAnchor {
                block_index,
                start_utf16,
                end_utf16,
                quote,
            }))
        }
        _ => Err(StatusCode::UNPROCESSABLE_ENTITY),
    }
}

pub(super) async fn authorized(
    state: &GrantState,
    headers: &HeaderMap,
    chapter: PositiveId,
    purchase: PositiveId,
    version: PositiveId,
) -> Result<(i64, [u8; 32]), StatusCode> {
    if state.reader_s3_origin.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let id = state
        .accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let user_id = PositiveId::new(id).map_err(|_| StatusCode::UNAUTHORIZED)?;
    match state
        .entitlement
        .resolve_entitlement(Actor { user_id }, chapter, Some(purchase))
        .await
    {
        Ok(EntitlementDecision::Allowed { pinned })
            if pinned.chapter_id == chapter && pinned.version_id == version =>
        {
            Ok((id, digest))
        }
        Ok(_) => Err(StatusCode::FORBIDDEN),
        Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) fn private_json(value: serde_json::Value) -> Response {
    let mut response = private_response(Json(value), &[]);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

pub(super) async fn list(
    State(state): State<GrantState>,
    Path(chapter_raw): Path<String>,
    headers: HeaderMap,
    Query(q): Query<CommentQuery>,
) -> Response {
    let (Some(chapter), Some(purchase), Some(version)) = (
        positive(&chapter_raw),
        positive(&q.purchase_id),
        positive(&q.version_id),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let after = match q.after {
        Some(v) => match positive(&v) {
            Some(id) => Some(id.get()),
            None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        },
        None => None,
    };
    let (actor, digest) = match authorized(&state, &headers, chapter, purchase, version).await {
        Ok(auth) => auth,
        Err(status) => return deny(status),
    };
    let storage = PgPaidChapterComments::new(state.accounts.pool().clone());
    match storage
        .list(
            actor,
            &digest,
            (purchase.get(), chapter.get(), version.get()),
            after,
            25,
        )
        .await
    {
        Ok(Some(page)) => private_json(json!({
            "version_id": version.get().to_string(),
            "items": page.items.into_iter().map(|c| json!({
                "id": c.id, "parent_id": c.parent_id, "body": c.body,
                "anchor": c.anchor.map(|a| json!({
                    "block_index": a.block_index,
                    "start_utf16": a.start_utf16, "end_utf16": a.end_utf16
                }))
            })).collect::<Vec<_>>(),
            "next_cursor": page.next_cursor
        })),
        Ok(None) => deny(StatusCode::FORBIDDEN),
        Err(ListError::Invalid) => deny(StatusCode::UNPROCESSABLE_ENTITY),
        Err(ListError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn submit(
    State(state): State<GrantState>,
    Path(chapter_raw): Path<String>,
    headers: HeaderMap,
    Json(input): Json<CommentInput>,
) -> Response {
    if headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Some(chapter), Some(purchase), Some(version)) = (
        positive(&chapter_raw),
        positive(&input.purchase_id),
        positive(&input.version_id),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let (actor, digest) = match authorized(&state, &headers, chapter, purchase, version).await {
        Ok(auth) => auth,
        Err(status) => return deny(status),
    };
    let body = match CommentBody::new(input.body.clone()) {
        Ok(body) => body,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let key = match CommentIntentKey::new(input.command_key.clone()) {
        Ok(key) => key,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let anchor = match parsed_anchor(&input) {
        Ok(anchor) => anchor,
        Err(status) => return deny(status),
    };
    let command_digest: [u8; 32] = Sha256::digest(key.as_str().as_bytes()).into();
    match state
        .accounts
        .permit_comment_action(actor, &command_digest)
        .await
    {
        Ok(ScopedActionAdmission::Allowed) => {}
        Ok(ScopedActionAdmission::Limited { .. }) => return deny(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let scope = match anchor {
        Some(a) => format!(
            "block:{}\nstart:{}\nend:{}\nquote:{}",
            a.block_index, a.start_utf16, a.end_utf16, a.quote
        ),
        None => "chapter-only".to_owned(),
    };
    let hash: [u8; 32] = Sha256::digest(format!(
        "chapter:{}\nversion:{}\nparent:{:?}\n{}\n{}",
        chapter.get(),
        version.get(),
        input.parent_id,
        scope,
        body.as_str()
    ))
    .into();
    let comments = PgPaidChapterComments::new(state.accounts.pool().clone());
    match comments
        .create(
            actor,
            &digest,
            purchase.get(),
            NewChapterComment {
                chapter_id: chapter.get(),
                version_id: version.get(),
                body: body.as_str(),
                command_key: key.as_str(),
                payload_hash: &hash,
                parent_id: input.parent_id,
                anchor,
            },
        )
        .await
    {
        CreateResult::Created(id) => private_json(json!({"status":"sent","id":id})),
        CreateResult::Replayed(id) => private_json(json!({"status":"repeat","id":id})),
        CreateResult::Invalid => deny(StatusCode::UNPROCESSABLE_ENTITY),
        CreateResult::Conflict => deny(StatusCode::CONFLICT),
        CreateResult::NotFound => deny(StatusCode::FORBIDDEN),
        CreateResult::Unavailable => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s011_paid_comments_refuse_reply_anchor_or_extra_authority() {
        let json = r#"{"purchase_id":"5","version_id":"9","body":"Привет",
            "command_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#;
        assert!(serde_json::from_str::<CommentInput>(json).is_ok());
        assert!(serde_json::from_str::<CommentInput>(&json.replace(
            "\"version_id\":\"9\"",
            "\"version_id\":\"9\",\"user_id\":99"
        ))
        .is_err());
        let mut form = serde_json::from_str::<CommentInput>(json).unwrap();
        form.parent_id = Some(2);
        form.anchor_block = Some(1);
        form.anchor_start = Some(0);
        form.anchor_end = Some(1);
        form.anchor_quote = Some("П".to_owned());
        assert!(parsed_anchor(&form).is_err());
    }
}
