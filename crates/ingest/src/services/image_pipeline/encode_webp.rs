//! Explicit lossless WebP, matching the encoder actually used by NEwBOO.
//! Historical quality=80/85 settings were ignored; no hidden faux-quality knob.
use super::ImagePipelineError as Error;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ExtendedColorType};

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
