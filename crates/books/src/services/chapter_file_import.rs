//! H-09 / S023: bounded, deterministic text import, with no side effects.
//! The result can only become a private draft after an explicit PG command.
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const MAX_IMPORT_BYTES: usize = 700_000;
pub const MAX_IMPORT_CHAPTERS: usize = 500;
pub const MAX_CHAPTER_CHARS: usize = 50_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedChapter {
    pub number: i32,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportError {
    Empty,
    InvalidFilename,
    InvalidEncoding,
    TooLarge,
    NoChapters,
    TooManyChapters,
    InvalidNumber,
    MissingTitle,
    EmptyBody,
    DuplicateNumber,
    UnexpectedPreamble,
}

pub fn valid_filename(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    !filename.is_empty()
        && filename.len() <= 128
        && !filename.chars().any(char::is_control)
        && !filename.contains('/')
        && !filename.contains('\\')
        && (lower.ends_with(".txt") || lower.ends_with(".md"))
}

/// NEwBOO-style numbered headings, optionally Markdown or === decorated.
/// A plain "Глава 1:" is an error; decorative "=== Глава 1 ===" has a default title.
fn heading(line: &str) -> Result<Option<(i32, String)>, ImportError> {
    let value = line.trim();
    let decorated = value.starts_with('=') && value.ends_with('=');
    let without_decor = if decorated {
        value.trim_matches('=').trim()
    } else {
        value
    };
    let hashes = without_decor.chars().take_while(|ch| *ch == '#').count();
    if hashes > 6 {
        return Ok(None);
    }
    let raw = without_decor[hashes..].trim_start();
    let mut chars = raw.chars();
    let keyword: String = chars.by_ref().take(5).collect();
    if keyword.to_lowercase() != "глава" {
        return Ok(None);
    }
    let rest = chars.as_str();
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return Ok(None);
    }
    let rest = rest.trim_start();
    let digits = rest
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return Ok(None);
    }
    let number: i32 = rest[..digits]
        .parse()
        .map_err(|_| ImportError::InvalidNumber)?;
    if !(1..=1_000_000).contains(&number) {
        return Err(ImportError::InvalidNumber);
    }
    let suffix = rest[digits..].trim_start();
    if suffix.is_empty() && decorated {
        return Ok(Some((number, format!("Глава {number}"))));
    }
    let mut suffix_chars = suffix.chars();
    let sep = suffix_chars.next().ok_or(ImportError::MissingTitle)?;
    if !matches!(sep, ':' | '-' | '–' | '—' | '.' | ')') {
        return Ok(None);
    }
    let title = suffix_chars.as_str().trim();
    if title.is_empty() {
        return Err(ImportError::MissingTitle);
    }
    Ok(Some((number, format!("Глава {number}{sep} {title}"))))
}

fn separator(line: &str) -> bool {
    let value = line.trim();
    value.len() >= 10 && value.bytes().all(|byte| byte == b'=')
}

fn append(
    chapters: &mut Vec<ImportedChapter>,
    active: &mut Option<(i32, String, Vec<String>)>,
) -> Result<(), ImportError> {
    if let Some((number, title, lines)) = active.take() {
        let body = lines.join("\n").trim().to_owned();
        if body.is_empty() {
            return Err(ImportError::EmptyBody);
        }
        if body.chars().count() > MAX_CHAPTER_CHARS || title.chars().count() > 200 {
            return Err(ImportError::TooLarge);
        }
        chapters.push(ImportedChapter {
            number,
            title,
            body,
        });
        if chapters.len() > MAX_IMPORT_CHAPTERS {
            return Err(ImportError::TooManyChapters);
        }
    }
    Ok(())
}

pub fn parse_text(filename: &str, text: &str) -> Result<Vec<ImportedChapter>, ImportError> {
    if !valid_filename(filename) {
        return Err(ImportError::InvalidFilename);
    }
    if text.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::TooLarge);
    }
    if text.contains('\u{fffd}') || text.contains('\0') {
        return Err(ImportError::InvalidEncoding);
    }
    if text.trim().is_empty() {
        return Err(ImportError::Empty);
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut chapters = Vec::new();
    let mut active: Option<(i32, String, Vec<String>)> = None;
    let mut numbers = HashSet::new();
    for line in normalized.lines() {
        if let Some((number, title)) = heading(line)? {
            if let Some((old_number, old_title, body)) = &active {
                if *old_number == number
                    && *old_title == title
                    && body.iter().all(|part| part.trim().is_empty())
                {
                    continue;
                }
            }
            if !numbers.insert(number) {
                return Err(ImportError::DuplicateNumber);
            }
            append(&mut chapters, &mut active)?;
            active = Some((number, title, Vec::new()));
        } else if let Some((number, title, body)) = &mut active {
            if separator(line) {
                continue;
            }
            let duplicate = body.iter().all(|part| part.trim().is_empty())
                && (line.trim().eq_ignore_ascii_case(title)
                    || line.trim().to_lowercase() == format!("глава {number}"));
            if !duplicate {
                body.push(line.trim_end().to_owned());
            }
        } else if !line.trim().is_empty() && !separator(line) {
            return Err(ImportError::UnexpectedPreamble);
        }
    }
    append(&mut chapters, &mut active)?;
    if chapters.is_empty() {
        return Err(ImportError::NoChapters);
    }
    Ok(chapters)
}

/// Stable, length-prefixed digest of exactly what PostgreSQL will write.
/// Book id is bound to the preview, so an unchanged preview cannot target another book.
pub fn fingerprint(book_id: i64, chapters: &[ImportedChapter]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"tor-chapter-import-v1");
    hash.update(book_id.to_be_bytes());
    hash.update((chapters.len() as u64).to_be_bytes());
    for chapter in chapters {
        hash.update(chapter.number.to_be_bytes());
        for value in [&chapter.title, &chapter.body] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newboo_headings_and_crlf_are_normalized_without_loss() {
        let chapters = parse_text(
            "chapters.TXT",
            "# Глава 1: Начало\r\nГлава 1: Начало\r\nТекст\r\n==========\r\nГЛАВА 2 — Продолжение\r\nГлава магов\r\nФинал",
        )
        .unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].body, "Текст");
        assert_eq!(chapters[1].title, "Глава 2— Продолжение");
        assert!(chapters[1].body.contains("Глава магов"));
    }

    #[test]
    fn decorated_chapter_and_utf8_markdown() {
        let chapters = parse_text(
            "hello.md",
            "==== Глава 1 ====\nТекст\n# Глава 2: Второй\nДалее",
        )
        .unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title, "Глава 1");
        assert_eq!(chapters[1].number, 2);
    }

    #[test]
    fn bad_files_and_chapters_fail_closed() {
        for (name, source, expected) in [
            ("bad.zip", "Глава 1: A\ntext", ImportError::InvalidFilename),
            (
                "../bad.txt",
                "Глава 1: A\ntext",
                ImportError::InvalidFilename,
            ),
            (
                "bad.txt",
                "Скрытая потеря\nГлава 1: A\ntext",
                ImportError::UnexpectedPreamble,
            ),
            ("bad.txt", "Глава 1:\nbody", ImportError::MissingTitle),
            ("bad.txt", "Глава 1: A\n", ImportError::EmptyBody),
            (
                "bad.txt",
                "Глава 1: A\nbody\nГлава 1: B\nbody",
                ImportError::DuplicateNumber,
            ),
            (
                "bad.txt",
                "Глава 1: A\nbad \u{fffd}",
                ImportError::InvalidEncoding,
            ),
        ] {
            assert_eq!(parse_text(name, source), Err(expected));
        }
        assert_eq!(parse_text("empty.txt", " "), Err(ImportError::Empty));
        assert_eq!(
            parse_text("empty.txt", "текст без заголовка"),
            Err(ImportError::UnexpectedPreamble)
        );
    }

    #[test]
    fn fingerprint_is_book_and_content_bound() {
        let one = parse_text("one.txt", "Глава 1: A\nbody").unwrap();
        let other = parse_text("other.txt", "Глава 1: A\nother").unwrap();
        assert_eq!(fingerprint(1, &one), fingerprint(1, &one));
        assert_ne!(fingerprint(1, &one), fingerprint(2, &one));
        assert_ne!(fingerprint(1, &one), fingerprint(1, &other));
    }
}
