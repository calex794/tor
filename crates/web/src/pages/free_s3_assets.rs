//! G/H static S3 reader scripts without embedding chapter bodies.
use axum::{http::header, response::IntoResponse};

/// S349: browser-only free chapter direct S3 loader.
pub async fn free_s3_reader_script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../../assets/newboo-free-s3-reader.js"),
    )
}

/// S349: progress engine is loaded only once the S3 body has been verified.
pub async fn free_s3_progress_script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../../assets/newboo-free-progress.js"),
    )
}

