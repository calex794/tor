//! S033 personal materials: separate from public Book, purchases and teams.
#![forbid(unsafe_code)]
use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PersonalError {
    Invalid,
    Missing,
    Unavailable,
}
impl fmt::Display for PersonalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for PersonalError {}

#[derive(Debug, Clone)]
pub struct PersonalBook {
    pub id: i64,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct PersonalChapterSummary {
    pub id: i64,
    pub chapter_no: i32,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct PersonalChapter {
    pub id: i64,
    pub book_id: i64,
    pub chapter_no: i32,
    pub title: String,
    pub content_markdown: String,
}

pub fn clean_book(title: &str, description: &str) -> Result<(String, String), PersonalError> {
    let title = title.trim();
    let description = description.trim();
    if title.is_empty() || title.chars().count() > 200 || description.chars().count() > 10_000 {
        return Err(PersonalError::Invalid);
    }
    Ok((title.to_owned(), description.to_owned()))
}

pub fn clean_chapter(title: &str, content: &str) -> Result<(String, String), PersonalError> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 || content.chars().count() > 500_000 {
        return Err(PersonalError::Invalid);
    }
    Ok((title.to_owned(), content.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s033_rejects_empty_and_oversized_materials() {
        assert_eq!(clean_book(" ", "").unwrap_err(), PersonalError::Invalid);
        assert_eq!(clean_book("a", &"x".repeat(10_001)).unwrap_err(), PersonalError::Invalid);
        assert_eq!(clean_chapter("a", &"x".repeat(500_001)).unwrap_err(), PersonalError::Invalid);
        assert_eq!(clean_book("  Личное  ", "  Черновик "), Ok(("Личное".into(), "Черновик".into())));
        assert!(clean_chapter("Глава", "").is_ok());
    }
}
