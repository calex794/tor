//! One CSP-safe, static chapter file import script.
use axum::{http::header, response::IntoResponse};

pub async fn script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../assets/newboo-chapter-import.js"),
    )
}
