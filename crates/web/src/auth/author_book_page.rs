//! Owner-only book management: keyset-paged chapters without giant pages.
use super::{deny, session_user, AuthState};
use crate::{
    auth_cookies::private_response,
    author_chapter_pages::{self, OwnerChapterPageOptions},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_books::services::authoring::{AuthorError, OwnedChapterCursor};

#[derive(Deserialize, Default)]
pub(super) struct AuthorBookQuery {
    after: Option<String>,
}

/// Accept only positive decimal (chapter_number, chapter_id) keysets.
fn chapter_cursor(raw: Option<&str>) -> Result<Option<OwnedChapterCursor>, ()> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let (number, id) = raw.split_once(':').ok_or(())?;
    if number.is_empty()
        || id.is_empty()
        || !number.bytes().all(|value| value.is_ascii_digit())
        || !id.bytes().all(|value| value.is_ascii_digit())
    {
        return Err(());
    }
    let number: i32 = number.parse().map_err(|_| ())?;
    let id: i64 = id.parse().map_err(|_| ())?;
    if number <= 0 || id <= 0 {
        return Err(());
    }
    Ok(Some(OwnedChapterCursor { number, id }))
}

/// The owned-book guard precedes every chapter read, including malformed cursors.
pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    Query(query): Query<AuthorBookQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.owned_book(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let cursor = match chapter_cursor(query.after.as_deref()) {
        Ok(cursor) => cursor,
        Err(()) => return deny(StatusCode::BAD_REQUEST),
    };
    let published = match state.authoring.owned_book_is_public(actor, id).await {
        Ok(published) => published,
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let demand_count = match state.demand.current_count(id).await {
        Ok(count) => count,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let page = match state
        .authoring
        .list_owned_chapters_page(actor, id, cursor)
        .await
    {
        Ok(page) => page,
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let latest_cover = if let Some(jobs) = state.cover_jobs.as_ref() {
        match jobs.latest_for_author(id, actor).await {
            Ok(latest) => latest,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        None
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        author_chapter_pages::detail_with_covers_with_demand_page(
            &book,
            published,
            &page.chapters,
            &csrf,
            OwnerChapterPageOptions {
                uploads_enabled: state.cover_upload_enabled,
                latest_job: latest_cover.as_deref(),
                demand_count,
                discounts_enabled: state.paid_checkout_enabled,
                next: page.next,
                has_previous_page: cursor.is_some(),
            },
        ),
        &cookies,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b_12_author_cursor_never_accepts_unbounded_or_malformed_input() {
        assert_eq!(chapter_cursor(None), Ok(None));
        assert_eq!(
            chapter_cursor(Some("501:900")),
            Ok(Some(OwnedChapterCursor {
                number: 501,
                id: 900
            }))
        );
        for raw in [
            "",
            "0:10",
            "10:0",
            "-1:10",
            "1:-2",
            "1",
            "1:2:3",
            "1:abc",
            "+1:2",
            "1:2%22",
            "2147483648:1",
        ] {
            assert_eq!(chapter_cursor(Some(raw)), Err(()), "{raw}");
        }
    }
}
