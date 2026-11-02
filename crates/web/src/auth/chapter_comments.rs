//! S011: form POST for exact, currently published free chapter comments.
//! The repository rechecks release/access in the same transaction as write.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tor_accounts::services::security::session_token_digest;
use tor_discussions::{
    domain::comments::{CommentBody, CommentIntentKey},
    repositories::public_chapter_comments::{
        CreateResult, NewAnchor, NewChapterComment, PgPublicChapterComments,
    },
};

#[derive(Deserialize)]
pub(super) struct ChapterCommentForm {
    csrf_token: Option<String>,
    version_id: i64,
    body: String,
    command_key: String,
    parent_id: Option<i64>,
    anchor_block: Option<i32>,
    anchor_start: Option<i32>,
    anchor_end: Option<i32>,
    anchor_quote: Option<String>,
}

fn selected_anchor(form: &ChapterCommentForm) -> Result<Option<NewAnchor<'_>>, StatusCode> {
    match (
        form.anchor_block,
        form.anchor_start,
        form.anchor_end,
        form.anchor_quote.as_deref(),
    ) {
        (None, None, None, None) => Ok(None),
        (Some(block_index), Some(start_utf16), Some(end_utf16), Some(quote))
            if form.parent_id.is_none() =>
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

/// Key identity binds immutable edition, parent, position, selected text and
/// message; the same user's command key cannot silently retarget another text.
fn payload_hash(
    chapter_id: i64,
    version_id: i64,
    parent_id: Option<i64>,
    anchor: Option<NewAnchor<'_>>,
    body: &str,
) -> [u8; 32] {
    let scope = match anchor {
        Some(a) => format!(
            "block:{}\nstart:{}\nend:{}\nquote:{}",
            a.block_index, a.start_utf16, a.end_utf16, a.quote
        ),
        None => "chapter-only".to_owned(),
    };
    Sha256::digest(format!(
        "chapter:{chapter_id}\nversion:{version_id}\nparent:{parent_id:?}\n{scope}\n{body}"
    ))
    .into()
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ChapterCommentForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|value| state.cookies.valid_csrf(&headers, value))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let body = match CommentBody::new(form.body.clone()) {
        Ok(body) => body,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let key = match CommentIntentKey::new(form.command_key.clone()) {
        Ok(key) => key,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    if chapter_id <= 0
        || form.version_id <= 0
        || form.parent_id.is_some_and(|id| id <= 0)
    {
        return deny(StatusCode::NOT_FOUND);
    }
    let anchor = match selected_anchor(&form) {
        Ok(anchor) => anchor,
        Err(status) => return deny(status),
    };
    // Shared PostgreSQL limiter, not a per-process bypassable map.
    let throttle = session_token_digest(&format!("action:chapter_comment:user:{actor}"));
    let permitted = match state.accounts.permit_auth_attempt(&throttle, 30, 60).await {
        Ok(permitted) => permitted,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !permitted {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let hash = payload_hash(
        chapter_id,
        form.version_id,
        form.parent_id,
        anchor,
        body.as_str(),
    );
    let store = PgPublicChapterComments::new(state.accounts.pool().clone());
    let result = store
        .create_free(NewChapterComment {
            actor_id: actor,
            chapter_id,
            version_id: form.version_id,
            body: body.as_str(),
            command_key: key.as_str(),
            payload_hash: &hash,
            parent_id: form.parent_id,
            anchor,
        })
        .await;
    match result {
        CreateResult::Created(_) => private_response(
            Redirect::to(&format!("/reader/{chapter_id}?comment=sent#chapter-comments")),
            &[],
        ),
        CreateResult::Replayed(_) => private_response(
            Redirect::to(&format!("/reader/{chapter_id}?comment=repeat#chapter-comments")),
            &[],
        ),
        CreateResult::Invalid => deny(StatusCode::UNPROCESSABLE_ENTITY),
        CreateResult::Conflict => deny(StatusCode::CONFLICT),
        CreateResult::NotFound => deny(StatusCode::NOT_FOUND),
        CreateResult::Unavailable => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s011_anchor_is_exact_and_replies_cannot_forge_fragment_targets() {
        let form = ChapterCommentForm {
            csrf_token: None,
            version_id: 5,
            body: "hello".into(),
            command_key: "a".repeat(43),
            parent_id: None,
            anchor_block: Some(1),
            anchor_start: Some(1),
            anchor_end: Some(3),
            anchor_quote: Some("😀".into()),
        };
        let anchor = selected_anchor(&form).unwrap().unwrap();
        assert_eq!(anchor.start_utf16, 1);
        let hash = payload_hash(4, 5, None, Some(anchor), "hello");
        assert_ne!(hash, payload_hash(4, 6, None, Some(anchor), "hello"));
        assert_ne!(hash, payload_hash(4, 5, Some(9), Some(anchor), "hello"));
        assert_ne!(hash, payload_hash(4, 5, None, None, "hello"));
        assert_ne!(
            hash,
            payload_hash(4, 5, None, Some(anchor), "different")
        );
        let mut reply = form;
        reply.parent_id = Some(9);
        assert!(selected_anchor(&reply).is_err());
        reply.anchor_block = None;
        reply.anchor_start = None;
        reply.anchor_end = None;
        reply.anchor_quote = None;
        assert!(selected_anchor(&reply).unwrap().is_none());
    }
}
