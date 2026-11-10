//! G-09/#748: gated, administrator-only private S3 audit console.
//! No routes or S3 clients in a build without the private paid-storage feature.
use super::AuthState;
use axum::Router;

#[cfg(feature = "s3-paid")]
mod enabled;

#[cfg(not(feature = "s3-paid"))]
pub(super) fn router() -> Router<AuthState> {
    Router::new()
}

#[cfg(feature = "s3-paid")]
pub(super) fn router() -> Router<AuthState> {
    enabled::router()
}
