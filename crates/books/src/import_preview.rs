//! H-09 / S023: deterministic, side-effect-free NEwBOO-compatible text preview.
//! This parser never publishes chapters or accesses PostgreSQL/S3.
use std::collections::HashSet;

pub const MAX_IMPORT_CHAPTERS: usize = 500;
pub const MAX_CHAPTER_CHARS: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedChapter {
    pub number: u32,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    Empty,
    MissingTitle(u32),
    NoChapters,
    TooManyChapters,
    DuplicateNumber(u32),
    InvalidNumber,
    BodyTooLong(u32),
}

fn heading(line: &str) -> Result<Option<(u32, String)>, ImportError> {
    let trimmed = line.trim();
    let without_hash = trimmed.trim_start_matches('#').trim_start();
    let Some(prefix) = without_hash.get(.."глава".len()) else {
        return Ok(None);
    };
    if !prefix.eq_ignore_ascii_case("глава") {
        return Ok(None);
    }
    let rest = without_hash["глава".len()..].trim_start();
    let digit_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digit_len == 0 {
        return Ok(None);
    }
    let number: u32 = rest[..digit_len]
        .parse()
        .map_err(|_| ImportError::InvalidNumber)?;
    if number == 0 {
        return Err(ImportError::InvalidNumber);
    }
    let tail = rest[digit_len..].trim_start();
    let mut chars = tail.chars();
    let Some(separator) = chars.next() else {
        return Err(ImportError::MissingTitle(number));
    };
    if !matches!(separator, ':' | '-' | '–' | '—' | '.' | ')') {
        return Ok(None);
    }
    let name = chars.as_str().trim();
    if name.is_empty() {
        return Err(ImportError::MissingTitle(number));
    }
    Ok(Some((number, format!("Глава {number}{separator} {name}"))))
}

fn finish(
    result: &mut Vec<ImportedChapter>,
    current: &mut Option<(u32, String, Vec<String>)>,
) -> Result<(), ImportError> {
    if let Some((number, title, lines)) = current.take() {
        let body = lines.join("\n").trim().to_owned();
        if body.chars().count() > MAX_CHAPTER_CHARS {
            return Err(ImportError::BodyTooLong(number));
        }
        result.push(ImportedChapter { number, title, body });
        if result.len() > MAX_IMPORT_CHAPTERS {
            return Err(ImportError::TooManyChapters);
        }
    }
    Ok(())
}

pub fn preview_plain_text(input: &str) -> Result<Vec<ImportedChapter>, ImportError> {
    if input.trim().is_empty() {
        return Err(ImportError::Empty);
    }
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut result = Vec::new();
    let mut current: Option<(u32, String, Vec<String>)> = None;
    for line in normalized.lines() {
        if let Some((number, title)) = heading(line)? {
            // NEwBOO suppresses a repeated heading immediately before the body.
            if let Some((old_number, old_title, body)) = &current {
                if *old_number == number
                    && *old_title == title
                    && body.iter().all(|line| line.trim().is_empty())
                {
                    continue;
                }
            }
            finish(&mut result, &mut current)?;
            current = Some((number, title, Vec::new()));
        } else if let Some((number, title, body)) = &mut current {
            let duplicate = body.iter().all(|line| line.trim().is_empty())
                && (line.trim().eq_ignore_ascii_case(title)
                    || line.trim().eq_ignore_ascii_case(&format!("Глава {number}")));
            if !duplicate && !(line.trim().len() >= 10 && line.trim().bytes().all(|b| b == b'=')) {
                body.push(line.to_owned());
            }
        }
    }
    finish(&mut result, &mut current)?;
    if result.is_empty() {
        return Err(ImportError::NoChapters);
    }
    let mut numbers = HashSet::new();
    for chapter in &result {
        if !numbers.insert(chapter.number) {
            return Err(ImportError::DuplicateNumber(chapter.number));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_newboo_headings_and_deduplicates_leading_title() {
        let chapters = preview_plain_text("# Глава 1: Начало\r\nГлава 1: Начало\r\nТекст\r\nГлава 2 — Продолжение\r\nДалее").unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].body, "Текст");
        assert_eq!(chapters[1].number, 2);
    }

    #[test]
    fn rejects_duplicate_chapter_numbers() {
        assert_eq!(preview_plain_text("Глава 1: А\nА\nГлава 1: Б\nБ"), Err(ImportError::DuplicateNumber(1)));
    }

    #[test]
    fn rejects_missing_title_and_empty_file() {
        assert_eq!(preview_plain_text("Глава 1:"), Err(ImportError::MissingTitle(1)));
        assert_eq!(preview_plain_text("  "), Err(ImportError::Empty));
    }
}
