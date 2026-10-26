//! Adapted from NEwBOO image-worker at commit 17125ef758a791e9630505c14d24f7fe65aed228.
//! Explicitly lossless WebP: the original implementation ignored "quality=80/85".
//! This library performs no I/O, does not build S3 keys and does not authorize uploads.
mod decode;
mod encode_webp;
mod transform;
mod validate;

mod orchestrator;
pub use orchestrator::{process_cover, ImageVariant, COVER_CARD, COVER_MAIN, RECIPE_VERSION};

/// Limits for an untrusted uploaded cover; independent of HTTP Content-Length.
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SOURCE_SIDE: u32 = 5_000;
pub const MAX_SOURCE_PIXELS: u64 = 24_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImagePipelineError {
    EmptySource,
    SourceTooLarge,
    UnsupportedFormat,
    ContentTypeMismatch,
    InvalidContainer,
    MetadataForbidden,
    AnimationForbidden,
    TrailingData,
    DimensionsExceeded,
    Decode(String),
    Transform(String),
    Encode(String),
}

impl std::fmt::Display for ImagePipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ImagePipelineError {}
