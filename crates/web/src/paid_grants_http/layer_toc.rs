//! D-05: authenticated Layer CURRENT navigation metadata.
//! This endpoint issues no purchases, text, private keys or storage grants.
use super::*;
use serde::Serialize;
use tor_commerce::{
    repositories::layer_entitlements::toc::LayerTocAccess,
    services::reader_views::{MixedTocCursor, MixedTocSide},
};

#[derive(Deserialize)]
pub(super) struct LayerTocQuery {
    before: Option<String>,
    after: Option<String>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct Item {
    chapter_id: String,
    chapter_no: i32,
    title: String,
    access_level: &'static str,
}

#[derive(Serialize)]
struct Page {
    items: Vec<Item>,
    next_cursor: Option<String>,
}

pub(super) async fn page(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    Query(query): Query<LayerTocQuery>,
    headers: HeaderMap,
) -> Response {
    if !state.layer_reader_enabled || state.reader_s3_origin.is_none() {
        return deny(StatusCode::NOT_FOUND);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let actor = match state.accounts
        .active_session_user(&session_token_digest(&token)).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (Some(chapter_id), Ok(actor_id)) =
        (positive(&raw_chapter), PositiveId::new(actor))
    else {
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
    let limit = query.limit.unwrap_or(36);
    if !(1..=60).contains(&limit) {
        return deny(StatusCode::BAD_REQUEST);
    }
    // Re-check the concrete current Layer chapter, not a book-level flag.
    let window = match state.layer
        .toc_window(actor_id, chapter_id, cursor, side, limit).await {
        Ok(page) => page,
        Err(error) => return deny(status(error)),
    };
    let items = window.chapters.into_iter().map(|chapter| Item {
        chapter_id: chapter.chapter_id.to_string(),
        chapter_no: chapter.chapter_no,
        title: chapter.title,
        access_level: match chapter.access {
            LayerTocAccess::Subscription => "layer",
            LayerTocAccess::PublicFree => "free",
        },
    }).collect();
    private_response(Json(Page { items, next_cursor: window.next_cursor }), &[])
}
