//! Strict container validation adapted from NEwBOO's image security policy.
//! Data must be checked before the image crate performs any decompression.
use super::{ImagePipelineError as Error, MAX_SOURCE_BYTES, MAX_SOURCE_PIXELS, MAX_SOURCE_SIDE};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum InputFormat {
    Jpeg,
    Png,
    Webp,
}
impl InputFormat {
    fn mime_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

pub(crate) fn validate_input(bytes: &[u8], declared_content_type: &str) -> Result<(), Error> {
    if bytes.is_empty() {
        return Err(Error::EmptySource);
    }
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(Error::SourceTooLarge);
    }
    let format = if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        InputFormat::Jpeg
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        InputFormat::Png
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        InputFormat::Webp
    } else {
        return Err(Error::UnsupportedFormat);
    };
    if declared_content_type.trim().to_ascii_lowercase() != format.mime_type() {
        return Err(Error::ContentTypeMismatch);
    }
    match format {
        InputFormat::Jpeg => check_jpeg(bytes),
        InputFormat::Png => check_png(bytes),
        InputFormat::Webp => check_webp(bytes),
    }
}

fn dimensions(width: u32, height: u32) -> Result<(), Error> {
    if width == 0
        || height == 0
        || width > MAX_SOURCE_SIDE
        || height > MAX_SOURCE_SIDE
        || u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS
    {
        return Err(Error::DimensionsExceeded);
    }
    Ok(())
}

fn check_png(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() < 33 {
        return Err(Error::InvalidContainer);
    }
    let mut offset = 8_usize;
    let mut seen_ihdr = false;
    let mut seen_idat = false;
    loop {
        if bytes.len().saturating_sub(offset) < 12 {
            return Err(Error::InvalidContainer);
        }
        let size = u32::from_be_bytes(
            bytes[offset..offset + 4].try_into().map_err(|_| Error::InvalidContainer)?,
        ) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let end = offset
            .checked_add(12)
            .and_then(|v| v.checked_add(size))
            .ok_or(Error::InvalidContainer)?;
        if end > bytes.len() {
            return Err(Error::InvalidContainer);
        }
        if !seen_ihdr {
            if kind != b"IHDR" || size != 13 {
                return Err(Error::InvalidContainer);
            }
            dimensions(
                u32::from_be_bytes(bytes[offset + 8..offset + 12].try_into().unwrap()),
                u32::from_be_bytes(bytes[offset + 12..offset + 16].try_into().unwrap()),
            )?;
            seen_ihdr = true;
        } else if kind == b"IHDR" {
            return Err(Error::InvalidContainer);
        }
        if matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
            return Err(Error::AnimationForbidden);
        }
        if matches!(kind, b"eXIf" | b"iCCP" | b"tEXt" | b"iTXt" | b"zTXt") {
            return Err(Error::MetadataForbidden);
        }
        if !matches!(
            kind,
            b"IHDR" | b"PLTE" | b"IDAT" | b"IEND" | b"tRNS"
                | b"gAMA" | b"cHRM" | b"sRGB" | b"pHYs" | b"bKGD"
        ) {
            return Err(Error::MetadataForbidden);
        }
        if kind == b"IDAT" {
            seen_idat = true;
        }
        if kind == b"IEND" {
            if !seen_idat || size != 0 {
                return Err(Error::InvalidContainer);
            }
            return if end == bytes.len() {
                Ok(())
            } else {
                Err(Error::TrailingData)
            };
        }
        offset = end;
    }
}

fn check_webp(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() < 20 {
        return Err(Error::InvalidContainer);
    }
    let riff_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if riff_size.checked_add(8) != Some(bytes.len()) {
        return Err(Error::TrailingData);
    }
    let mut offset = 12_usize;
    let mut seen_image = false;
    let mut size_known = false;
    while offset < bytes.len() {
        if bytes.len() - offset < 8 {
            return Err(Error::InvalidContainer);
        }
        let kind = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        let end = body.checked_add(size).ok_or(Error::InvalidContainer)?;
        let padded_end = end.checked_add(size % 2).ok_or(Error::InvalidContainer)?;
        if padded_end > bytes.len() {
            return Err(Error::InvalidContainer);
        }
        match kind {
            b"VP8X" => {
                if size < 10 {
                    return Err(Error::InvalidContainer);
                }
                if bytes[body] & 0b0000_0010 != 0 {
                    return Err(Error::AnimationForbidden);
                }
                let width = 1 + u32::from(bytes[body + 4])
                    + (u32::from(bytes[body + 5]) << 8)
                    + (u32::from(bytes[body + 6]) << 16);
                let height = 1 + u32::from(bytes[body + 7])
                    + (u32::from(bytes[body + 8]) << 8)
                    + (u32::from(bytes[body + 9]) << 16);
                dimensions(width, height)?;
                size_known = true;
            }
            b"VP8 " => {
                if size < 10 || &bytes[body + 3..body + 6] != b"\x9d\x01\x2a" {
                    return Err(Error::InvalidContainer);
                }
                let width = u16::from_le_bytes([bytes[body + 6], bytes[body + 7]]) & 0x3fff;
                let height = u16::from_le_bytes([bytes[body + 8], bytes[body + 9]]) & 0x3fff;
                dimensions(u32::from(width), u32::from(height))?;
                seen_image = true;
                size_known = true;
            }
            b"VP8L" => {
                if size < 5 || bytes[body] != 0x2f {
                    return Err(Error::InvalidContainer);
                }
                let bits =
                    u32::from_le_bytes(bytes[body + 1..body + 5].try_into().unwrap());
                dimensions((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)?;
                seen_image = true;
                size_known = true;
            }
            b"ALPH" => (),
            b"ANIM" | b"ANMF" => return Err(Error::AnimationForbidden),
            b"ICCP" | b"EXIF" | b"XMP " => return Err(Error::MetadataForbidden),
            _ => return Err(Error::MetadataForbidden),
        }
        offset = padded_end;
    }
    if !size_known || !seen_image {
        return Err(Error::InvalidContainer);
    }
    Ok(())
}

fn check_jpeg(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() < 6 || !bytes.ends_with(&[0xff, 0xd9]) {
        return Err(Error::TrailingData);
    }
    let mut pos = 2_usize;
    let mut seen_sof = false;
    while pos < bytes.len() {
        if bytes[pos] != 0xff {
            return Err(Error::InvalidContainer);
        }
        while pos < bytes.len() && bytes[pos] == 0xff {
            pos += 1;
        }
        let marker = *bytes.get(pos).ok_or(Error::InvalidContainer)?;
        pos += 1;
        if marker == 0xd9 {
            return if seen_sof && pos == bytes.len() {
                Ok(())
            } else {
                Err(Error::InvalidContainer)
            };
        }
        if marker == 0xda {
            // The decoder validates the entropy-coded scan; the EOI must end the file.
            return if seen_sof {
                Ok(())
            } else {
                Err(Error::InvalidContainer)
            };
        }
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if marker == 0xfe || (0xe1..=0xef).contains(&marker) {
            return Err(Error::MetadataForbidden);
        }
        if bytes.len().saturating_sub(pos) < 2 {
            return Err(Error::InvalidContainer);
        }
        let segment_length = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        let end = pos.checked_add(segment_length).ok_or(Error::InvalidContainer)?;
        if segment_length < 2 || end > bytes.len() {
            return Err(Error::InvalidContainer);
        }
        if matches!(
            marker,
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf
        ) {
            if segment_length < 7 {
                return Err(Error::InvalidContainer);
            }
            dimensions(
                u32::from(u16::from_be_bytes([bytes[pos + 5], bytes[pos + 6]])),
                u32::from(u16::from_be_bytes([bytes[pos + 3], bytes[pos + 4]])),
            )?;
            seen_sof = true;
        }
        pos = end;
    }
    Err(Error::InvalidContainer)
}
