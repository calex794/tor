//! D-scoped paid table of contents. No chapter text, grants or signed URLs.
use super::*;
use serde::Serialize;
use tor_commerce::services::reader_views::{
    MixedTocAccess, MixedTocCursor, MixedTocSide,
};

#[derive(Deserialize)]
pub(super) struct PaidTocQuery {
    purchase_id: String,
    before: Option<String>,
    after: Option<String>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct ChapterItem {
    chapter_id: String,
    chapter_no: i32,
    title: String,
    access_level: &'static str,
    purchase_id: Option<String>,
    version_no: Option<i32>,
    price_minor: Option<i64>,
}

#[derive(Serialize)]
struct WindowResponse {
    items: Vec<ChapterItem>,
    next_cursor: Option<String>,
}

pub(super) async fn page(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    Query(query): Query<PaidTocQuery>,
    headers: HeaderMap,
) -> Response {
    if state.reader_s3_origin.is_none() {
        return deny(StatusCode::NOT_FOUND);
    }
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let actor = match state.accounts.active_session_user(&session_token_digest(&session)).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (Some(chapter_id), Some(purchase_id), Ok(actor_id)) = (
        positive(&raw_chapter),
        positive(&query.purchase_id),
        PositiveId::new(actor),
    ) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let (cursor, side) = match (query.before.as_deref(), query.after.as_deref()) {
        (Some(raw), None) => (MixedTocCursor::parse(raw), MixedTocSide::Before),
        (None, Some(raw)) => (MixedTocCursor::parse(raw), MixedTocSide::After),
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let Some(cursor) = cursor else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let limit = query.limit.unwrap_or(60);
    if !(1..=100).contains(&limit) {
        return deny(StatusCode::BAD_REQUEST);
    }
    // The anchor is an actual live D grant, not a public or inferred purchase.
    match state.entitlement.resolve_entitlement(
        Actor { user_id: actor_id }, chapter_id, Some(purchase_id),
    ).await {
        Ok(EntitlementDecision::Allowed { pinned }) if pinned.chapter_id == chapter_id => {},
        Ok(EntitlementDecision::Allowed { .. }) => return deny(StatusCode::CONFLICT),
        Ok(EntitlementDecision::Denied { reason }) => return deny(status(match reason {
            tor_contracts::paid_delivery::DenialReason::Hidden => DeliveryError::Hidden,
            tor_contracts::paid_delivery::DenialReason::Unknown => DeliveryError::Unavailable,
            _ => DeliveryError::Denied,
        })),
        Err(error) => return deny(status(error)),
    }
    let window = match state.entitlement.mixed_book_toc_window(
        actor_id, chapter_id, purchase_id, cursor, side, limit,
    ).await {
        Ok(window) => window,
        Err(error) => return deny(status(error)),
    };
    let items = window.chapters.into_iter().map(|chapter| {
        let (access_level, purchase_id, version_no, price_minor) = match chapter.access {
            MixedTocAccess::Purchased { purchase_id, version_no, .. } =>
                ("purchased", Some(purchase_id.to_string()), Some(version_no), None),
            MixedTocAccess::PublicFree => ("free", None, None, None),
            MixedTocAccess::Locked { price_minor } =>
                ("locked", None, None, price_minor),
        };
        ChapterItem {
            chapter_id: chapter.chapter_id.to_string(),
            chapter_no: chapter.chapter_no,
            title: chapter.title, access_level, purchase_id, version_no, price_minor,
        }
    }).collect();
    private_response(Json(WindowResponse {
        items, next_cursor: window.next_cursor,
    }), &[])
}
