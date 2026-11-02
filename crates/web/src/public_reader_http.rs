//! Public chapter reading and the S005 paid metadata paywall.
use super::{auth_cookies, pages, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
};
use tor_reading::services::PublicReadingRepository;

/// C-10: successful and denied reading both use no-store and strict CSP.
pub(super) async fn reader_page(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    let Some(reader) = state.reader else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    let chapter = match reader.public_chapter(id).await {
        Ok(Some(chapter)) => chapter,
        Ok(None) => match reader.public_paid_chapter_preview(id).await {
            Ok(Some(preview)) => {
                // Publicly visible paid metadata has a paywall, never plaintext.
                // Hidden, held, deleted and unreleased chapters still yield 404.
                let page = if state.paid_checkout_browser {
                    pages::paid_chapter_paywall_checkout(&preview, Some(id))
                } else {
                    pages::paid_chapter_paywall(&preview)
                };
                let mut response = auth_cookies::private_response(page, &[]);
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
    let mut progress_csrf = None;
    let mut progress_cookies = Vec::new();
    if method == Method::GET {
        if let Some(catalog) = state.postgres.as_ref() {
            // If verification is unavailable, omit optional analytics. Never
            // misclassify an authenticated reader as an anonymous visitor.
            let actor = match state.follow_control.as_ref() {
                Some(control) => match control.verified_reader(&headers).await {
                    Ok(actor) => actor,
                    Err(_) => {
                        return auth_cookies::private_response(
                            pages::reader_with_login_link(&chapter, Some(id)),
                            &[],
                        )
                    }
                },
                None => None,
            };
            // Only verified sessions may receive a CSRF pair for the existing
            // actor-bound PUT /api/v1/me/progress/free endpoint.
            if actor.is_some() {
                if let Some(control) = state.follow_control.as_ref() {
                    let (_, policy) = control.browser_context();
                    let (csrf, cookies) = policy.csrf_for_page(&headers);
                    progress_csrf = Some(csrf);
                    progress_cookies = cookies;
                }
            }
            if let Ok(chapter_id) = i64::try_from(id) {
                if let Err(error) =
                    tor_reading::repositories::book_stats::PgBookStats::new(catalog.pool())
                        .record_open(chapter_id, actor)
                        .await
                {
                    tracing::warn!(?error, "book statistics open not recorded");
                }
            }
        }
    }
    let mut response = auth_cookies::private_response(
        pages::reader_with_optional_progress(&chapter, Some(id), progress_csrf.as_deref()),
        &progress_cookies,
    );
    // Never enable scripts for paid, hidden or denied content.
    response.headers_mut().insert(
        "content-security-policy",
        axum::http::HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response
}
