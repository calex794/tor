//! Center crop + Lanczos3 resize, adapted from NEwBOO image-worker at 17125ef.
//! Sequential targets by default: a shared 1-GiB VPS should not spawn Rayon
//! jobs and duplicate decoded 24-MP buffers for two small cover variants.
use super::ImagePipelineError as Error;
use fast_image_resize as fr;
use image::{DynamicImage, RgbaImage};
use std::num::NonZeroU32;

pub(crate) fn resize_to_fill(
    image: &DynamicImage,
    target_w: u32,
    target_h: u32,
) -> Result<DynamicImage, Error> {
    if target_w == 0 || target_h == 0 {
        return Err(Error::Transform("zero target dimensions".into()));
    }
    let src_w = image.width();
    let src_h = image.height();
    let src_ratio = src_w as f32 / src_h as f32;
    let dst_ratio = target_w as f32 / target_h as f32;
    let (crop_w, crop_h) = if src_ratio > dst_ratio {
        ((src_h as f32 * dst_ratio).round() as u32, src_h)
    } else {
        (src_w, (src_w as f32 / dst_ratio).round() as u32)
    };
    let crop_w = crop_w.clamp(1, src_w);
    let crop_h = crop_h.clamp(1, src_h);
    let cropped = image
        .crop_imm((src_w - crop_w) / 2, (src_h - crop_h) / 2, crop_w, crop_h)
        .to_rgba8();
    let width = NonZeroU32::new(crop_w).ok_or_else(|| Error::Transform("invalid width".into()))?;
    let height = NonZeroU32::new(crop_h).ok_or_else(|| Error::Transform("invalid height".into()))?;
    let source = fr::Image::from_vec_u8(width, height, cropped.into_raw(), fr::PixelType::U8x4)
        .map_err(|e| Error::Transform(e.to_string()))?;
    let dest_w = NonZeroU32::new(target_w).ok_or_else(|| Error::Transform("invalid width".into()))?;
    let dest_h = NonZeroU32::new(target_h).ok_or_else(|| Error::Transform("invalid height".into()))?;
    let mut dest = fr::Image::new(dest_w, dest_h, fr::PixelType::U8x4);
    let mut resizer = fr::Resizer::new(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3));
    resizer
        .resize(&source.view(), &mut dest.view_mut())
        .map_err(|e| Error::Transform(e.to_string()))?;
    let rgba = RgbaImage::from_raw(target_w, target_h, dest.into_vec())
        .ok_or_else(|| Error::Transform("invalid RGBA buffer".into()))?;
    Ok(DynamicImage::ImageRgba8(rgba))
}

pub(crate) fn cascade_resize_base(
    image: &DynamicImage,
    width: u32,
    height: u32,
) -> Result<DynamicImage, Error> {
    let mut current = image.clone();
    while (current.width() as f32 / width as f32) > 2.5
        && (current.height() as f32 / height as f32) > 2.5
    {
        let w = ((current.width() as f32 * 0.4).round() as u32).max(width);
        let h = ((current.height() as f32 * 0.4).round() as u32).max(height);
        current = resize_to_fill(&current, w, h)?;
    }
    resize_to_fill(&current, width, height)
}
