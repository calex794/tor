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
                    auth_cookies::private_response(
                        pages::paid_chapter_paywall_checkout(
                            &preview,
                            state.paid_checkout_browser.then_some(id),
                        ),
                        &[],
                    );
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
    auth_cookies::private_response(pages::reader(&chapter), &[])
}
