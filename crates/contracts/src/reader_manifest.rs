//! Immutable parser v1: the same UTF-16 block boundaries in B, C and H.
//! Metadata contains no paid plaintext and never grants an entitlement.
#![forbid(unsafe_code)]

pub const READER_PARSER_SCHEMA_V1: i16 = 1;

/// Match the browser's 200-block grouping, including newline normalization.
pub fn reader_blocks(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let paragraphs: Vec<&str> = normalized.split("\n\n").collect();
    let group_size = paragraphs.len().div_ceil(200);
    paragraphs
        .chunks(group_size)
        .map(|group| group.join("\n\n"))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderVersionManifestV1 {
    pub block_utf16_lengths: Vec<i32>,
    pub last_readable_block_index: i32,
    pub last_readable_char_offset: i32,
}

impl ReaderVersionManifestV1 {
    /// Build only from the B-owned immutable Markdown, never client offsets.
    pub fn from_markdown(text: &str) -> Option<Self> {
        let blocks = reader_blocks(text);
        if blocks.is_empty() || blocks.len() > 200 {
            return None;
        }
        let block_utf16_lengths = blocks
            .iter()
            .map(|b| i32::try_from(b.encode_utf16().count()).ok())
            .collect::<Option<Vec<_>>>()?;
        if block_utf16_lengths.iter().any(|len| *len > 100_000) {
            return None;
        }
        let (last_index, _) = blocks
            .iter()
            .enumerate()
            .rev()
            .find(|(_, block)| !block.trim().is_empty())?;
        Some(Self {
            last_readable_block_index: i32::try_from(last_index + 1).ok()?,
            last_readable_char_offset: block_utf16_lengths[last_index],
            block_utf16_lengths,
        })
    }

    /// Check one bookmark without downloading ciphertext or trusting an
    /// unbounded client-provided offset.
    pub fn valid_place(&self, block_index: i32, char_offset: i32) -> bool {
        if !(0..=100_000).contains(&char_offset) {
            return false;
        }
        usize::try_from(block_index)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.block_utf16_lengths.get(i))
            .is_some_and(|len| char_offset <= *len)
    }

    pub fn is_completed_place(&self, block_index: i32, char_offset: i32) -> bool {
        self.valid_place(block_index, char_offset)
            && block_index == self.last_readable_block_index
            && char_offset == self.last_readable_char_offset
    }
}
