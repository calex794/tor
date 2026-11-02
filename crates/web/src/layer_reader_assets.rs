//! Static Layer CURRENT reader script, separate from the core page registry.
use axum::{http::header, response::IntoResponse};

pub(crate) async fn script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("../assets/newboo-layer-reader.mjs"),
    )
}
