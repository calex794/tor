//! Bounded decode based on NEwBOO image-worker/src/image_pipeline/decode.rs.
use super::{ImagePipelineError as Error, MAX_SOURCE_PIXELS, MAX_SOURCE_SIDE};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};
use std::io::Cursor;

pub(crate) fn decode_image(bytes: &[u8]) -> Result<DynamicImage, Error> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    decoder
        .set_limits(limits)
        .map_err(|_| Error::DimensionsExceeded)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return Err(Error::DimensionsExceeded);
    }
    DynamicImage::from_decoder(decoder).map_err(|e| Error::Decode(e.to_string()))
}
