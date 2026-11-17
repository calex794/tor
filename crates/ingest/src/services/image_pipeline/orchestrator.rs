//! Pure G-05 cover recipes. Do not use on paid chapter assets (different ACL).
use super::decode::decode_image;
use super::encode_webp::{encode_webp_lossless, encode_webp_lossy};
use super::transform::{cascade_resize_base, resize_to_fill};
use super::validate::validate_input;
use super::ImagePipelineError;

pub const COVER_MAIN: (&str, u32, u32) = ("main", 380, 570);
pub const COVER_CARD: (&str, u32, u32) = ("card", 150, 220);
/// Stable v1 recipe for already-enqueued cover jobs and their idempotent retries.
pub const LEGACY_RECIPE_VERSION: &str = "newboo-cover-lossless-webp-v1";
/// A new recipe is required whenever encoding or output changes.
pub const RECIPE_VERSION: &str = "tor-cover-webp-lossy-q85-v2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageVariant {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub content_type: &'static str,
    /// v2 uses lossy WebP for opaque art, lossless WebP for transparency.
    /// Historic v1 jobs always use lossless WebP.
    pub encoding: &'static str,
    pub bytes: Vec<u8>,
}

/// Legacy-compatible entry point for direct callers (including package covers).
/// Book-cover worker jobs use process_cover_with_recipe and a stored recipe.
/// Caller remains responsible for storage, authorization and job fencing.
pub fn process_cover(
    source: &[u8],
    declared_content_type: &str,
) -> Result<Vec<ImageVariant>, ImagePipelineError> {
    process_cover_with_recipe(source, declared_content_type, LEGACY_RECIPE_VERSION)
}

/// Preserve old queued jobs: their recorded v1 recipe must never silently
/// switch encoding when the worker binary is upgraded or config is toggled.
pub fn process_cover_with_recipe(
    source: &[u8],
    declared_content_type: &str,
    recipe_version: &str,
) -> Result<Vec<ImageVariant>, ImagePipelineError> {
    let lossy = match recipe_version {
        RECIPE_VERSION => true,
        LEGACY_RECIPE_VERSION => false,
        _ => return Err(ImagePipelineError::Encode("unknown cover recipe".into())),
    };

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
        // Alpha-bearing covers retain their alpha exactly; WebP lossy would
        // preserve alpha but can change nearby RGB and introduce halos.
        let has_transparency = lossy && resized.to_rgba8().pixels().any(|p| p.0[3] != 255);
        let (bytes, encoding) = if lossy && !has_transparency {
            (encode_webp_lossy(&resized)?, "webp-lossy-q85")
        } else {
            (encode_webp_lossless(&resized)?, "webp-lossless")
        };
        outputs.push(ImageVariant {
            name,
            width: w,
            height: h,
            content_type: "image/webp",
            encoding,
            bytes,
        });
    }
    Ok(outputs)
}
