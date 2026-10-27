//! B/C canonical reader segmentation: identical UTF-16 bounds on immutable V1.
use tor_contracts::reader_manifest::{
    reader_blocks, ReaderVersionManifestV1, READER_PARSER_SCHEMA_V1,
};

#[test]
fn unicode_crlf_and_trailing_blank_are_not_false_completions() {
    assert_eq!(READER_PARSER_SCHEMA_V1, 1);
    let md = "А😀Б\r\n\r\nКонец😀\r\n\r\n";
    let blocks = reader_blocks(md);
    assert_eq!(blocks, vec!["А😀Б", "Конец😀", ""]);
    let manifest = ReaderVersionManifestV1::from_markdown(md).unwrap();
    assert_eq!(manifest.block_utf16_lengths, vec![4, 7, 0]);
    assert_eq!(
        (
            manifest.last_readable_block_index,
            manifest.last_readable_char_offset
        ),
        (2, 7)
    );
    assert!(manifest.valid_place(1, 4));
    assert!(manifest.valid_place(3, 0));
    assert!(!manifest.valid_place(1, 5));
    assert!(!manifest.valid_place(0, 0));
    assert!(!manifest.valid_place(2, 8));
    assert!(!manifest.is_completed_place(3, 0));
    assert!(!manifest.is_completed_place(2, 6));
    assert!(manifest.is_completed_place(2, 7));
}

#[test]
fn exactly_200_anchors_cover_thousands_of_paragraphs() {
    let md = (0..1200)
        .map(|i| format!("Paragraph {i}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let blocks = reader_blocks(&md);
    assert_eq!(blocks.len(), 200);
    assert_eq!(blocks.join("\n\n"), md);
    let manifest = ReaderVersionManifestV1::from_markdown(&md).unwrap();
    assert_eq!(manifest.block_utf16_lengths.len(), 200);
    assert!(manifest.is_completed_place(200, blocks.last().unwrap().encode_utf16().count() as i32));
    assert!(!manifest.valid_place(201, 0));
}

#[test]
fn empty_or_oversized_last_block_cannot_become_a_read_fact() {
    for md in ["", " \n\n", "\n\n\t"] {
        assert!(ReaderVersionManifestV1::from_markdown(md).is_none());
    }
    assert!(ReaderVersionManifestV1::from_markdown(&"A".repeat(100_001)).is_none());
    let manifest = ReaderVersionManifestV1::from_markdown("Начало\n\nПоследний").unwrap();
    assert!(!manifest.valid_place(1, -1));
    assert!(!manifest.valid_place(2, 100_001));
    assert!(!manifest.is_completed_place(1, 6));
}
