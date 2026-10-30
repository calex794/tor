//! Public chapter reading and the S005 paid metadata paywall.
use super::{auth_cookies, pages, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use tor_reading::services::PublicReadingRepository;

/// C-10: successful and denied reading both use no-store and strict CSP.
pub(super) async fn reader_page(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    let Some(reader) = state.reader else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    let chapter = match reader.public_chapter(id).await {
        Ok(Some(chapter)) => chapter,
        Ok(None) => match reader.public_paid_chapter_preview(id).await {
            Ok(Some(preview)) => {
                // Publicly visible paid metadata has a paywall, never plaintext.
                // Hidden, held, deleted and unreleased chapters still yield 404.
                let mut response =
                    auth_cookies::private_response(pages::paid_chapter_paywall(&preview), &[]);
                response.headers_mut().insert(
                    "x-robots-tag",
                    axum::http::HeaderValue::from_static("noindex, nofollow"),
                );
                return response;
            }
            Ok(None) => return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
            Err(_) => return auth_cookies::private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
        },
        Err(_) => return auth_cookies::private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    if !tor_reading::domain::renderer::valid_reader_length(&chapter.body_markdown) {
        return auth_cookies::private_response(StatusCode::PAYLOAD_TOO_LARGE, &[]);
    }
    if let Some(catalog) = state.postgres.as_ref() {
        if let Ok(chapter_id) = i64::try_from(id) {
            if let Err(error) = tor_reading::repositories::book_stats::PgBookStats::new(
                catalog.pool(),
            ).record_open(chapter_id, None).await {
                tracing::warn!(?error, "anonymous book statistics open not recorded");
            }
        }
    }
    auth_cookies::private_response(pages::reader_with_login_link(&chapter, Some(id)), &[])
}
