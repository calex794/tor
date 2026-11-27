//! AUTH-READ-02a: deterministic, bounded visible-text derivative.
//! This is not a content authority. The caller must bind the result to the
//! exact verified normalized source and edition before persisting it.
use crate::reader_text::{valid_reader_length, visible_text};
use unicode_segmentation::UnicodeSegmentation;

pub const GUEST_PREFIX_ALGORITHM: &str = "GUEST-PREFIX-1";
pub const MAX_PREFIX_SCALARS: usize = 2_000;
pub const MAX_PREFIX_BYTES: usize = 8_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestPrefixError {
    SourceTooLong,
    UnsupportedLiteralMarkup,
    InvalidPercent,
}

/// Only one maximum 50% prefix and its total visible scalar count survive
/// construction. Full source and full visible text are never fields.
pub struct GuestPrefix {
    visible_scalar_count: usize,
    prefix: String,
}

fn limit(total: usize, percent: u8) -> usize {
    (total.saturating_mul(usize::from(percent)) / 100)
        .min(MAX_PREFIX_SCALARS)
        .min(total.saturating_sub(1))
}

/// Choose only extended-grapheme boundaries, using the fixed dependency
/// version for this algorithm. A nearby sentence/word boundary is preferred
/// for guest output, never at the expense of exceeding the scalar limit.
fn cut(text: &str, cap: usize, prefer_natural: bool) -> usize {
    let mut count = 0;
    let mut end = 0;
    let mut natural = (0, 0);
    for (start, grapheme) in text.grapheme_indices(true) {
        let len = grapheme.chars().count();
        if count + len > cap {
            break;
        }
        if grapheme.chars().all(char::is_whitespace) && start > 0 {
            natural = (start, count);
        }
        count += len;
        end = start + grapheme.len();
        if grapheme
            .chars()
            .last()
            .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '…'))
        {
            natural = (end, count);
        }
    }
    if prefer_natural
        && natural.0 > 0
        && count - natural.1 <= 64
        && natural.1 >= cap.saturating_mul(4) / 5
    {
        natural.0
    } else {
        end
    }
}

/// The historical renderer deliberately displays unsupported markup as
/// literal text. Never send that literal image/link destination or HTML-like
/// attribute to a preview. This conservative v1 returns CTA-only for such
/// chapters rather than invent a second Markdown parser or fetch an asset.
fn unsupported_literal(text: &str) -> bool {
    text.contains("![")
        || text.contains("](")
        || text.match_indices('<').any(|(index, _)| {
            text[index + 1..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic() || matches!(ch, '/' | '!' | '?'))
        })
}

impl GuestPrefix {
    pub fn from_source(source: &str) -> Result<Self, GuestPrefixError> {
        if !valid_reader_length(source) {
            return Err(GuestPrefixError::SourceTooLong);
        }
        let visible = visible_text(source);
        if unsupported_literal(&visible) {
            return Err(GuestPrefixError::UnsupportedLiteralMarkup);
        }
        let visible_scalar_count = visible.chars().count();
        // Keep the entire strictly bounded grapheme prefix internally. The
        // natural-boundary preference is applied only at guest selection.
        let end = cut(&visible, limit(visible_scalar_count, 50), false);
        let prefix = visible[..end].to_owned();
        debug_assert!(prefix.len() <= MAX_PREFIX_BYTES);
        Ok(Self {
            visible_scalar_count,
            prefix,
        })
    }

    pub fn visible_scalar_count(&self) -> usize {
        self.visible_scalar_count
    }

    /// Internal persistence input only: never serialize this maximum prefix
    /// into a guest response. The future PG adapter must bind source+digest.
    pub fn maximum_prefix(&self) -> &str {
        &self.prefix
    }

    /// Plain text, not Markdown or HTML. The web adapter must HTML-escape
    /// once, never auto-link or reparse this string as source markup.
    pub fn excerpt(&self, percent: u8) -> Result<&str, GuestPrefixError> {
        if !matches!(percent, 10 | 20 | 30 | 50) {
            return Err(GuestPrefixError::InvalidPercent);
        }
        let end = cut(&self.prefix, limit(self.visible_scalar_count, percent), true);
        Ok(&self.prefix[..end])
    }
}
