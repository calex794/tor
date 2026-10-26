//! Pure G-05 cover recipe. Do not use on paid chapter assets (different ACL).
use super::decode::decode_image;
use super::encode_webp::encode_webp_lossless;
use super::transform::{cascade_resize_base, resize_to_fill};
use super::validate::validate_input;
use super::ImagePipelineError;

pub const COVER_MAIN: (&str, u32, u32) = ("main", 380, 570);
pub const COVER_CARD: (&str, u32, u32) = ("card", 150, 220);
/// Changing the output algorithm/encoding must increment this recipe version.
pub const RECIPE_VERSION: &str = "newboo-cover-lossless-webp-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageVariant {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub content_type: &'static str,
    /// Always lossless; historical WebP "quality=80/85" was not honored.
    pub encoding: &'static str,
    pub bytes: Vec<u8>,
}

/// Decode, center-crop and generate two deterministic WebP cover sizes.
///
/// No async, S3, SQL or user-provided object key. Call from G's bounded
/// spawn_blocking worker only after A/B ownership + G source verification.
/// A durable PG job, staging upload and B cover-slot commit are separate tasks.
pub fn process_cover(
    source: &[u8],
    declared_content_type: &str,
) -> Result<Vec<ImageVariant>, ImagePipelineError> {
    validate_input(source, declared_content_type)?;
    let image = decode_image(source)?;
    // Preserve NEwBOO's cascading downscale for extremely large originals.
    let max_scale = (image.width() as f32 / COVER_MAIN.1 as f32)
        .max(image.height() as f32 / COVER_MAIN.2 as f32)
        .max(image.width() as f32 / COVER_CARD.1 as f32)
        .max(image.height() as f32 / COVER_CARD.2 as f32);
    let base = if max_scale > 4.0 {
        cascade_resize_base(&image, COVER_MAIN.1, COVER_MAIN.2)?
    } else {
        image
    };
    let mut outputs = Vec::with_capacity(2);
    for (name, w, h) in [COVER_MAIN, COVER_CARD] {
        let resized = if base.width() == w && base.height() == h {
            base.clone()
        } else {
            resize_to_fill(&base, w, h)?
        };
        outputs.push(ImageVariant {
            name,
            width: w,
            height: h,
            content_type: "image/webp",
            encoding: "webp-lossless",
            bytes: encode_webp_lossless(&resized)?,
        });
    }
    Ok(outputs)
}
