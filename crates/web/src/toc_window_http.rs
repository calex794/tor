//! Metadata-only TOC window HTTP endpoint. Does not issue reader entitlements.
use super::*;

#[derive(Default, Deserialize)]
struct TocWindowQuery {
    around_chapter_id: Option<u64>,
    before: Option<String>,
    after: Option<String>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct TocWindowResponse {
    items: Vec<TocItem>,
    before_cursor: Option<String>,
    after_cursor: Option<String>,
    total_chapters: Option<i64>,
}

pub(super) async fn book_toc_window(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<TocWindowQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    use tor_reading::services::toc::{TocCursor, TocWindowPosition};
    let position = match (
        query.around_chapter_id,
        query.before.as_deref(),
        query.after.as_deref(),
    ) {
        (Some(id), None, None) if id > 0 => TocWindowPosition::Around(id),
        (None, Some(raw), None) => match TocCursor::parse(raw) {
            Some(cursor) => TocWindowPosition::Before(cursor),
            None => return auth_cookies::private_response(StatusCode::BAD_REQUEST, &[]),
        },
        (None, None, Some(raw)) => match TocCursor::parse(raw) {
            Some(cursor) => TocWindowPosition::After(cursor),
            None => return auth_cookies::private_response(StatusCode::BAD_REQUEST, &[]),
        },
        _ => return auth_cookies::private_response(StatusCode::BAD_REQUEST, &[]),
    };
    let Some(reader) = state.reader.as_ref() else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    let digest = public_reader_http::reader_session_digest(&state, &headers);
    let page = match reader
        .public_toc_window_for_session(id, position, query.limit.unwrap_or(99), digest.as_ref())
        .await
    {
        Ok(Some(page)) => page,
        Ok(None) => return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
        Err(error) => return auth_cookies::private_response(reading_status(error), &[]),
    };
    let items = page
        .chapters
        .into_iter()
        .map(|chapter| TocItem {
            chapter_id: chapter.id.to_string(),
            chapter_no: chapter.number,
            title: chapter.title,
            access_level: if chapter.is_paid { "paid" } else { "free" },
        })
        .collect();
    // TOC is a viewer-scoped projection. Never share or cache an age-gated response.
    let response = Json(TocWindowResponse {
        items,
        before_cursor: page.before_cursor,
        after_cursor: page.after_cursor,
        total_chapters: page.total_chapters,
    })
    .into_response();
    let mut response = response;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    response
}
