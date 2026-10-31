//! S023: safe, bounded ZIP of complete numbered chapter TXT/MD/DOCX files.
//! Metadata alone is never trusted for allocation or decompression size.
use crate::services::{
    chapter_file_import::{parse_text, ImportError, MAX_IMPORT_BYTES},
    docx_import::{extract_docx_text, MAX_DOCX_BYTES},
};
use encoding_rs::WINDOWS_1251;
use std::{
    cmp::Ordering,
    collections::HashSet,
    io::{Cursor, Read},
};

pub const MAX_ZIP_BYTES: usize = 1_250_000;
const MAX_MEMBERS: usize = 100;
const MAX_MEMBER_BYTES: u64 = 1_250_000;
const MAX_UNCOMPRESSED_BYTES: u64 = 16 * 1024 * 1024;
const MAX_COMPRESSION_RATIO: u64 = 200;

pub fn valid_zip_filename(filename: &str) -> bool {
    !filename.is_empty()
        && filename.len() <= 128
        && filename.to_ascii_lowercase().ends_with(".zip")
        && !filename.chars().any(char::is_control)
        && !filename.contains('/')
        && !filename.contains('\\')
}

fn safe_member_path(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !(name.len() >= 2 && name.as_bytes()[1] == b':')
        && name
            .trim_end_matches('/')
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
}

fn junk_member(name: &str) -> bool {
    name.starts_with("__MACOSX/")
        || name
            .rsplit('/')
            .next()
            .is_some_and(|base| base == ".DS_Store" || base.starts_with("._"))
}

/// Natural numeric order, so chapter2.txt precedes chapter10.txt.
fn natural_compare(left: &str, right: &str) -> Ordering {
    let l = left.to_lowercase();
    let r = right.to_lowercase();
    let (a, b) = (l.as_bytes(), r.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (start_i, start_j) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let a_number = &a[start_i..i];
            let b_number = &b[start_j..j];
            let a_nonzero = a_number
                .iter()
                .position(|byte| *byte != b'0')
                .unwrap_or(a_number.len());
            let b_nonzero = b_number
                .iter()
                .position(|byte| *byte != b'0')
                .unwrap_or(b_number.len());
            let na = &a_number[a_nonzero..];
            let nb = &b_number[b_nonzero..];
            let order = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if order != Ordering::Equal {
                return order;
            }
            let order = a_number.len().cmp(&b_number.len());
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a[i].cmp(&b[j]);
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    a.len()
        .saturating_sub(i)
        .cmp(&b.len().saturating_sub(j))
        .then_with(|| left.cmp(right))
}

fn decode_text(bytes: &[u8], encoding: &str) -> Result<String, ImportError> {
    match encoding {
        "utf-8" => {
            let value = std::str::from_utf8(bytes).map_err(|_| ImportError::InvalidEncoding)?;
            Ok(value.trim_start_matches('\u{feff}').to_owned())
        }
        "windows-1251" => {
            let decoded = WINDOWS_1251
                .decode_without_bom_handling_and_without_replacement(bytes)
                .ok_or(ImportError::InvalidEncoding)?;
            Ok(decoded.into_owned())
        }
        _ => Err(ImportError::InvalidEncoding),
    }
}

/// Decode all supported members before the shared preview and PG transaction.
/// Reject any unrecognised member instead of silently omitting a chapter.
pub fn extract_zip_text(source: &[u8], encoding: &str) -> Result<String, ImportError> {
    if source.is_empty() || source.len() > MAX_ZIP_BYTES {
        return Err(ImportError::TooLarge);
    }
    if !matches!(encoding, "utf-8" | "windows-1251") {
        return Err(ImportError::InvalidEncoding);
    }
    if !source.starts_with(b"PK\x03\x04") {
        return Err(ImportError::InvalidArchive);
    }
    let mut archive =
        zip::ZipArchive::new(Cursor::new(source)).map_err(|_| ImportError::InvalidArchive)?;
    if archive.is_empty() || archive.len() > MAX_MEMBERS {
        return Err(ImportError::InvalidArchive);
    }

    let mut total_uncompressed = 0_u64;
    let mut filenames = HashSet::new();
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| ImportError::InvalidArchive)?;
        let name = entry.name().to_owned();
        if !safe_member_path(&name)
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(ImportError::InvalidArchive);
        }
        if entry.is_dir() {
            continue;
        }
        let packed = entry.compressed_size();
        if entry.size() > MAX_MEMBER_BYTES
            || (packed == 0 && entry.size() != 0)
            || (packed != 0 && entry.size() / packed > MAX_COMPRESSION_RATIO)
        {
            return Err(ImportError::TooLarge);
        }
        total_uncompressed = total_uncompressed.saturating_add(entry.size());
        if total_uncompressed > MAX_UNCOMPRESSED_BYTES {
            return Err(ImportError::TooLarge);
        }
        if junk_member(&name) {
            continue;
        }
        let lowercase = name.to_lowercase();
        if lowercase.ends_with(".zip")
            || !(lowercase.ends_with(".txt")
                || lowercase.ends_with(".md")
                || lowercase.ends_with(".docx"))
            || !filenames.insert(lowercase.clone())
        {
            return Err(ImportError::InvalidArchive);
        }
        let mut data = Vec::new();
        entry
            .take(MAX_MEMBER_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|_| ImportError::InvalidArchive)?;
        if data.len() as u64 > MAX_MEMBER_BYTES {
            return Err(ImportError::TooLarge);
        }
        members.push((name, data));
    }
    if members.is_empty() {
        return Err(ImportError::NoChapters);
    }
    members.sort_by(|left, right| natural_compare(&left.0, &right.0));
    let mut assembled = String::new();
    for (name, bytes) in members {
        let part = if name.to_lowercase().ends_with(".docx") {
            if bytes.len() > MAX_DOCX_BYTES {
                return Err(ImportError::TooLarge);
            }
            extract_docx_text(&bytes)?
        } else {
            decode_text(&bytes, encoding)?
        };
        // Each file must be a complete chapter group: no accidental merging
        // of a headerless member with the previous member's body.
        parse_text("member.txt", &part)?;
        if assembled.len().saturating_add(part.len()).saturating_add(2) > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge);
        }
        assembled.push_str(&part);
        assembled.push('\n');
    }
    // Verify cross-member duplicate numbers and all canonical chapter limits.
    parse_text("assembled.txt", &assembled)?;
    Ok(assembled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn build_zip(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, content) in members {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_naturally_sorted_numbered_chapters() {
        let zip = build_zip(&[
            (
                "chapter10.txt",
                "Глава 10: Десятая\nТекст десятый".as_bytes(),
            ),
            (
                "folder/chapter2.md",
                "# Глава 2: Вторая\nТекст второй".as_bytes(),
            ),
            ("chapter1.txt", "Глава 1: Первая\nТекст первый".as_bytes()),
        ]);
        let text = extract_zip_text(&zip, "utf-8").unwrap();
        let chapters = parse_text("assembled.txt", &text).unwrap();
        assert_eq!(
            chapters
                .iter()
                .map(|chapter| chapter.number)
                .collect::<Vec<_>>(),
            vec![1, 10, 2]
        );
        // Natural sorting applies to the full member path; the folder member
        // is still intact and no chapter is silently skipped.
        assert_eq!(chapters.len(), 3);
        assert!(text.contains("Текст второй"));
    }

    #[test]
    fn direct_numeric_sequence_and_cp1251_decode() {
        let zip = build_zip(&[
            ("10.txt", "Глава 10: Десятая\nТекст".as_bytes()),
            ("2.txt", "Глава 2: Вторая\nТекст".as_bytes()),
            ("1.txt", "Глава 1: Первая\nТекст".as_bytes()),
        ]);
        let parsed = parse_text("all.txt", &extract_zip_text(&zip, "utf-8").unwrap()).unwrap();
        assert_eq!(
            parsed
                .iter()
                .map(|chapter| chapter.number)
                .collect::<Vec<_>>(),
            vec![1, 2, 10]
        );
        let cp1251 = build_zip(&[(
            "chapter1.txt",
            &[
                0xc3, 0xeb, 0xe0, 0xe2, 0xe0, 0x20, 0x31, 0x3a, 0x20, 0xd2, 0xe5, 0xf1, 0xf2, 0x0a,
                0xd2, 0xe5, 0xea, 0xf1, 0xf2,
            ],
        )]);
        assert_eq!(
            extract_zip_text(&cp1251, "utf-8"),
            Err(ImportError::InvalidEncoding)
        );
        assert_eq!(
            parse_text(
                "all.txt",
                &extract_zip_text(&cp1251, "windows-1251").unwrap()
            )
            .unwrap()[0]
                .number,
            1
        );
    }

    #[test]
    fn unsafe_members_and_duplicate_numbers_fail_atomically() {
        for name in ["../escape.txt", "/root.txt", "nested.zip", "./chapter.txt"] {
            let zip = build_zip(&[(name, "Глава 1: Вредная\nТекст".as_bytes())]);
            assert!(extract_zip_text(&zip, "utf-8").is_err());
        }
        let duplicate = build_zip(&[
            ("1.txt", "Глава 1: Первая\nТекст".as_bytes()),
            ("2.txt", "Глава 1: Другая\nТекст".as_bytes()),
        ]);
        assert_eq!(
            extract_zip_text(&duplicate, "utf-8"),
            Err(ImportError::DuplicateNumber)
        );
        let headerless = build_zip(&[
            ("1.txt", "Глава 1: Первая\nТекст".as_bytes()),
            ("2.txt", "без заголовка".as_bytes()),
        ]);
        assert_eq!(
            extract_zip_text(&headerless, "utf-8"),
            Err(ImportError::UnexpectedPreamble)
        );
    }

    #[test]
    fn oversized_archive_and_invalid_encoding_are_rejected() {
        assert_eq!(
            extract_zip_text(b"broken", "utf-8"),
            Err(ImportError::InvalidArchive)
        );
        assert_eq!(
            extract_zip_text(&vec![0; MAX_ZIP_BYTES + 1], "utf-8"),
            Err(ImportError::TooLarge)
        );
        let zip = build_zip(&[("1.txt", "Глава 1: Первая\nТекст".as_bytes())]);
        assert_eq!(
            extract_zip_text(&zip, "unknown"),
            Err(ImportError::InvalidEncoding)
        );
    }
}
