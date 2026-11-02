//! Static browser module for Layer-only protected text reading.
use axum::{http::header, response::IntoResponse};

pub async fn script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/newboo-layer-reader.mjs"),
    )
}
