//! WebP encoders for the cover recipe. Lossy is reserved for fully opaque
//! images; alpha-bearing images and jobs from the legacy recipe stay lossless.
use super::ImagePipelineError as Error;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ExtendedColorType};

/// Fixed in the recipe version; changing this parameter requires a new recipe.
pub(crate) const LOSSY_QUALITY: f32 = 85.0;

pub(crate) fn encode_webp_lossy(image: &DynamicImage) -> Result<Vec<u8>, Error> {
    let rgb = image.to_rgb8();
    let encoded = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height())
        .encode_simple(false, LOSSY_QUALITY)
        .map_err(|error| Error::Encode(format!("libwebp: {error:?}")))?;
    let bytes = encoded.to_vec();
    if bytes.is_empty() {
        return Err(Error::Encode("empty WebP lossy output".into()));
    }
    Ok(bytes)
}

pub(crate) fn encode_webp_lossless(image: &DynamicImage) -> Result<Vec<u8>, Error> {
    let rgba = image.to_rgba8();
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| Error::Encode(e.to_string()))?;
    Ok(out)
}
