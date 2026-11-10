//! Compatibility route for cached clients; individual attribution is retired.
use super::{api, Runtime};
use crate::auth_cookies::deny;
use axum::{
    extract::{Form, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
};

pub(super) async fn activate(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(body): Form<api::Activation>,
) -> Response {
    if let Err(status) = api::csrf(&state, &headers, &body.csrf_token) {
        return deny(status);
    }
    if let Err(status) = api::actor(&state, &headers).await {
        return deny(status);
    }
    if !tor_recommendations::domain::feedback::valid_token(&body.ticket) {
        return api::error(tor_recommendations::domain::feedback::FeedbackError::Invalid);
    }
    api::retired()
}

/// Keep the old static URL harmless for previously cached HTML.
pub(super) async fn script() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=300"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../../assets/organic_discovery.js"),
    )
        .into_response()
}
