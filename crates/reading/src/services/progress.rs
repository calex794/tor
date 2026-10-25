//! A reading place is a private cursor, never a purchase or an access grant.
#![forbid(unsafe_code)]

/// At most 200 stable anchors per immutable chapter version. Splitting and
/// grouping is deterministic and does not depend on viewport dimensions.
pub fn reader_blocks(text: &str) -> Vec<String> {
    // HTML parsers normalize CRLF and bare CR. Match the browser DOM text
    // before computing or validating its UTF-16 character offsets.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let paragraphs: Vec<&str> = normalized.split("\n\n").collect();
    let group_size = paragraphs.len().div_ceil(200);
    paragraphs
        .chunks(group_size)
        .map(|group| group.join("\n\n"))
        .collect()
}

/// Reading position in UTF-16 code units, matching the browser DOM Text.offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadingPlace {
    pub block_index: i32,
    pub char_offset: i32,
}

pub fn valid_text_place(place: ReadingPlace, blocks: &[String]) -> Result<(), ProgressError> {
    valid_place(place.block_index, blocks.len())?;
    let text = &blocks[(place.block_index - 1) as usize];
    if place.char_offset < 0
        || place.char_offset > 100_000
        || usize::try_from(place.char_offset)
            .ok()
            .is_none_or(|v| v > text.encode_utf16().count())
    {
        return Err(ProgressError::Invalid);
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ContinueReading {
    pub book_id: i64,
    pub book_title: String,
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub chapter_title: String,
    /// Zero when the saved version is no longer current; never invent a
    /// position in edited text.
    pub block_index: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressError {
    Unavailable,
    NotFound,
    StaleVersion,
    Invalid,
}

pub fn valid_place(block_index: i32, block_count: usize) -> Result<(), ProgressError> {
    if block_index <= 0
        || usize::try_from(block_index)
            .ok()
            .filter(|n| *n <= block_count)
            .is_none()
    {
        return Err(ProgressError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_cover_entire_text_without_loss() {
        let source = (0..1200)
            .map(|i| format!("Текст {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let blocks = reader_blocks(&source);
        assert_eq!(blocks.len(), 200);
        assert_eq!(blocks.join("\n\n"), source);
    }

    #[test]
    fn imported_windows_line_endings_match_browser_text() {
        assert_eq!(reader_blocks("A\r\n\r\nB\rC"), vec!["A", "B\nC"]);
    }

    #[test]
    fn position_uses_utf16_units_and_rejects_outside_text() {
        let text = reader_blocks("т😀a\n\nSecond");
        assert_eq!(text[0].encode_utf16().count(), 4);
        assert!(valid_text_place(
            ReadingPlace {
                block_index: 1,
                char_offset: 4
            },
            &text
        )
        .is_ok());
        assert_eq!(
            valid_text_place(
                ReadingPlace {
                    block_index: 1,
                    char_offset: 5
                },
                &text
            ),
            Err(ProgressError::Invalid)
        );
        assert_eq!(
            valid_text_place(
                ReadingPlace {
                    block_index: 0,
                    char_offset: 0
                },
                &text
            ),
            Err(ProgressError::Invalid)
        );
        assert_eq!(
            valid_text_place(
                ReadingPlace {
                    block_index: 1,
                    char_offset: -1
                },
                &text
            ),
            Err(ProgressError::Invalid)
        );
    }

    #[test]
    fn blocks_keep_original_html_as_data() {
        assert_eq!(
            reader_blocks("a\n\n<script>x</script>"),
            vec!["a", "<script>x</script>"]
        );
        assert_eq!(reader_blocks("single").len(), 1);
        assert!(valid_place(1, 1).is_ok());
        assert_eq!(valid_place(0, 1), Err(ProgressError::Invalid));
        assert_eq!(valid_place(201, 200), Err(ProgressError::Invalid));
    }
}
