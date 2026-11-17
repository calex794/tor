//! Deterministic real-format regression tests for the ported NEwBOO pipeline.
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use std::io::Cursor;
use tor_ingest::services::image_pipeline::{
    process_cover, process_cover_with_recipe, ImagePipelineError as Error, COVER_CARD, COVER_MAIN,
    LEGACY_RECIPE_VERSION, MAX_SOURCE_BYTES, RECIPE_VERSION,
};

fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, format).expect("encode fixture");
    cursor.into_inner()
}

fn colored_landscape() -> DynamicImage {
    // With center-crop-to-portrait, the red and blue edges disappear.
    let image = RgbImage::from_fn(120, 60, |x, _| {
        if x < 30 {
            Rgb([255, 0, 0])
        } else if x >= 90 {
            Rgb([0, 0, 255])
        } else {
            Rgb([0, 255, 0])
        }
    });
    DynamicImage::ImageRgb8(image)
}

#[test]
fn legacy_recipe_generates_both_decodable_lossless_webp_variants() {
    let original = encode(&colored_landscape(), ImageFormat::Png);
    let variants = process_cover_with_recipe(&original, "image/png", LEGACY_RECIPE_VERSION).unwrap();
    assert_eq!(variants.len(), 2);
    assert_eq!(LEGACY_RECIPE_VERSION, "newboo-cover-lossless-webp-v1");
    assert_eq!(RECIPE_VERSION, "tor-cover-webp-lossy-q85-v2");
    for (variant, expected) in variants.iter().zip([COVER_MAIN, COVER_CARD]) {
        assert_eq!((variant.name, variant.width, variant.height), expected);
        assert_eq!(variant.encoding, "webp-lossless");
        assert_eq!(variant.content_type, "image/webp");
        let decoded = image::load_from_memory_with_format(&variant.bytes, ImageFormat::WebP)
            .expect("read real encoded WebP");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (expected.1, expected.2)
        );
        let center = decoded
            .to_rgba8()
            .get_pixel(expected.1 / 2, expected.2 / 2)
            .0;
        assert_eq!(center, [0, 255, 0, 255]); // center of crop
    }
}

#[test]
fn opaque_book_art_uses_lossy_v2_while_existing_jobs_keep_lossless_v1() {
    // Deterministic high-frequency art: the lossy codec should be smaller,
    // and the legacy recipe must stay reproducible after a worker upgrade.
    let pixels = RgbImage::from_fn(380, 570, |x, y| {
        let n = (x.wrapping_mul(1_103_515_245) ^ y.wrapping_mul(2_654_435_761))
            .wrapping_add(x.wrapping_mul(y.wrapping_add(17)));
        Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
    });
    let original = encode(&DynamicImage::ImageRgb8(pixels), ImageFormat::Png);
    let lossy = process_cover(&original, "image/png").unwrap();
    let legacy = process_cover_with_recipe(&original, "image/png", LEGACY_RECIPE_VERSION).unwrap();
    assert_eq!(lossy.len(), 2);
    for (new, old) in lossy.iter().zip(legacy.iter()) {
        assert_eq!(new.encoding, "webp-lossy-q85");
        assert_eq!(old.encoding, "webp-lossless");
        assert_eq!(new.content_type, "image/webp");
        assert_eq!(old.content_type, "image/webp");
        assert_eq!((new.width, new.height), (old.width, old.height));
        let decoded = image::load_from_memory_with_format(&new.bytes, ImageFormat::WebP)
            .expect("decode libwebp lossy output");
        assert_eq!((decoded.width(), decoded.height()), (new.width, new.height));
        assert!(new.bytes.len() < old.bytes.len(), "lossy should beat v1 on photographic noise");
    }
    // This also verifies the old recipe has not silently changed on upgrade.
    let replay = process_cover_with_recipe(&original, "image/png", LEGACY_RECIPE_VERSION).unwrap();
    assert_eq!(legacy, replay);
}

#[test]
fn unknown_recipe_fails_closed_before_any_encoding() {
    let source = encode(&colored_landscape(), ImageFormat::Png);
    assert!(matches!(
        process_cover_with_recipe(&source, "image/png", "unknown-codec-v99"),
        Err(Error::Encode(_))
    ));
}

#[test]
fn real_jpeg_and_webp_sources_work() {
    let source = colored_landscape();
    let jpeg = encode(&source, ImageFormat::Jpeg);
    let webp = encode(&source, ImageFormat::WebP);
    for (mime, data) in [("image/jpeg", jpeg), ("image/webp", webp)] {
        let variants = process_cover(&data, mime).unwrap();
        assert_eq!(variants.len(), 2);
        assert!(variants.iter().all(|v| !v.bytes.is_empty()));
        assert_eq!(variants[0].width, 380);
        assert_eq!(variants[1].height, 220);
    }
}

#[test]
fn transparent_input_keeps_alpha_and_is_lossless() {
    let img = RgbaImage::from_pixel(60, 90, Rgba([15, 120, 200, 78]));
    let png = encode(&DynamicImage::ImageRgba8(img), ImageFormat::Png);
    let outputs = process_cover(&png, "image/png").unwrap();
    assert!(outputs.iter().all(|v| v.encoding == "webp-lossless"));
    let main = image::load_from_memory_with_format(&outputs[0].bytes, ImageFormat::WebP)
        .expect("decode lossless output")
        .to_rgba8();
    assert_eq!(main.get_pixel(190, 285).0, [15, 120, 200, 78]);
}

#[test]
fn rejects_unsupported_mismatched_and_oversized_sources() {
    assert_eq!(
        process_cover(&[], "image/png").unwrap_err(),
        Error::EmptySource
    );
    assert_eq!(
        process_cover(&vec![0; MAX_SOURCE_BYTES + 1], "image/png").unwrap_err(),
        Error::SourceTooLarge
    );
    assert_eq!(
        process_cover(b"not an image", "image/png").unwrap_err(),
        Error::UnsupportedFormat
    );
    let png = encode(&colored_landscape(), ImageFormat::Png);
    assert_eq!(
        process_cover(&png, "image/jpeg").unwrap_err(),
        Error::ContentTypeMismatch
    );
}

#[test]
fn rejects_png_metadata_animation_and_trailing_data() {
    let png = encode(&colored_landscape(), ImageFormat::Png);
    // Add an EXIF chunk before the terminal IEND; rejection precedes CRC checks.
    let mut exif = png[..png.len() - 12].to_vec();
    exif.extend_from_slice(&[0, 0, 0, 0]);
    exif.extend_from_slice(b"eXIf");
    exif.extend_from_slice(&[0, 0, 0, 0]);
    exif.extend_from_slice(&png[png.len() - 12..]);
    assert_eq!(
        process_cover(&exif, "image/png").unwrap_err(),
        Error::MetadataForbidden
    );
    let mut animated = png[..png.len() - 12].to_vec();
    animated.extend_from_slice(&[0, 0, 0, 0]);
    animated.extend_from_slice(b"acTL");
    animated.extend_from_slice(&[0, 0, 0, 0]);
    animated.extend_from_slice(&png[png.len() - 12..]);
    assert_eq!(
        process_cover(&animated, "image/png").unwrap_err(),
        Error::AnimationForbidden
    );
    let mut appended = png;
    appended.extend_from_slice(b"trailing");
    assert_eq!(
        process_cover(&appended, "image/png").unwrap_err(),
        Error::TrailingData
    );
}

#[test]
fn rejects_jpeg_exif_and_trailing_bytes() {
    let jpeg = encode(&colored_landscape(), ImageFormat::Jpeg);
    let mut with_exif = jpeg[..2].to_vec();
    with_exif.extend_from_slice(&[0xff, 0xe1, 0, 8]);
    with_exif.extend_from_slice(b"Exif\0\0");
    with_exif.extend_from_slice(&jpeg[2..]);
    assert_eq!(
        process_cover(&with_exif, "image/jpeg").unwrap_err(),
        Error::MetadataForbidden
    );
    let mut trailing = jpeg;
    trailing.push(0);
    assert_eq!(
        process_cover(&trailing, "image/jpeg").unwrap_err(),
        Error::TrailingData
    );
}

#[test]
fn rejects_webp_animation_and_trailing_bytes() {
    let mut webp = encode(&colored_landscape(), ImageFormat::WebP);
    webp.extend_from_slice(b"ANIM");
    webp.extend_from_slice(&0_u32.to_le_bytes());
    let length = (webp.len() - 8) as u32;
    webp[4..8].copy_from_slice(&length.to_le_bytes());
    assert_eq!(
        process_cover(&webp, "image/webp").unwrap_err(),
        Error::AnimationForbidden
    );
    webp.push(0);
    assert_eq!(
        process_cover(&webp, "image/webp").unwrap_err(),
        Error::TrailingData
    );
}

#[test]
fn oversized_png_header_is_rejected_before_allocating_pixels() {
    let mut png = encode(&colored_landscape(), ImageFormat::Png);
    png[16..20].copy_from_slice(&6_000_u32.to_be_bytes());
    assert_eq!(
        process_cover(&png, "image/png").unwrap_err(),
        Error::DimensionsExceeded
    );
}
