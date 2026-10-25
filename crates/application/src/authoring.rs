//! Validation and outputs for the first authenticated author journey.
#![forbid(unsafe_code)]
use bokarust_domain::books::BookKey;
use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthorError {
    Invalid,
    Forbidden,
    Missing,
    Conflict,
    Unavailable,
}
impl fmt::Display for AuthorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for AuthorError {}

pub fn valid_command_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Debug)]
pub struct NewBook {
    pub source_identity: String,
    pub target_language: String,
    pub title: String,
    pub command_key: String,
}

pub fn new_book(
    source: &str,
    language: &str,
    title: &str,
    command_key: &str,
) -> Result<NewBook, AuthorError> {
    let key = BookKey::new(source, language).map_err(|_| AuthorError::Invalid)?;
    let title = title.trim();
    if key.source_identity().chars().count() > 256
        || key.target_language().len() > 32
        || key.source_identity().chars().any(char::is_control)
        || !(1..=200).contains(&title.chars().count())
        || title.chars().any(char::is_control)
        || !valid_command_key(command_key)
    {
        return Err(AuthorError::Invalid);
    }
    Ok(NewBook {
        source_identity: key.source_identity().to_owned(),
        target_language: key.target_language().to_owned(),
        title: title.to_owned(),
        command_key: command_key.to_owned(),
    })
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ChapterAccess {
    Free,
    Paid,
}
impl ChapterAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Paid => "paid",
        }
    }
}

#[derive(Debug)]
pub struct SaveChapter {
    pub book_id: i64,
    pub chapter_id: Option<i64>,
    pub number: i32,
    pub expected_revision: i64,
    pub title: String,
    pub body: String,
    pub access: ChapterAccess,
    pub price_amount: i64,
    pub command_key: String,
}

pub struct SaveChapterRequest<'a> {
    pub book_id: u64,
    pub chapter_id: Option<u64>,
    pub number: i32,
    pub expected_revision: i64,
    pub title: &'a str,
    pub body: &'a str,
    pub access: &'a str,
    pub price_amount: i64,
    pub command_key: &'a str,
}

pub fn save_chapter(input: SaveChapterRequest<'_>) -> Result<SaveChapter, AuthorError> {
    let book_id = i64::try_from(input.book_id).map_err(|_| AuthorError::Invalid)?;
    let chapter_id = input
        .chapter_id
        .map(|id| i64::try_from(id).map_err(|_| AuthorError::Invalid))
        .transpose()?;
    let title = input.title.trim();
    let body = input.body.replace("\r\n", "\n");
    let access = match input.access {
        "free" => ChapterAccess::Free,
        "paid" => ChapterAccess::Paid,
        _ => return Err(AuthorError::Invalid),
    };
    if book_id <= 0
        || chapter_id.is_some_and(|id| id <= 0)
        || input.number <= 0
        || input.expected_revision < 0
        || (chapter_id.is_none() && input.expected_revision != 0)
        || !(1..=200).contains(&title.chars().count())
        || title.chars().any(char::is_control)
        || !(1..=50_000).contains(&body.chars().count())
        || body.contains('\0')
        || !valid_command_key(input.command_key)
        || match access {
            ChapterAccess::Free => input.price_amount != 0,
            ChapterAccess::Paid => input.price_amount <= 0,
        }
    {
        return Err(AuthorError::Invalid);
    }
    Ok(SaveChapter {
        book_id,
        chapter_id,
        number: input.number,
        expected_revision: input.expected_revision,
        title: title.to_owned(),
        body,
        access,
        price_amount: input.price_amount,
        command_key: input.command_key.to_owned(),
    })
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SavedChapter {
    pub chapter_id: i64,
    pub version_id: i64,
    pub revision: i64,
}

pub struct OwnedBook {
    pub id: i64,
    pub title: String,
}

pub struct OwnedChapterSummary {
    pub id: i64,
    pub number: i32,
    pub title: String,
    pub access: ChapterAccess,
}

pub struct OwnedChapter {
    pub number: i32,
    pub title: String,
    pub body: String,
    pub access: ChapterAccess,
    pub price_amount: i64,
    pub revision: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn source_language_and_request_key_are_canonical() {
        let book = new_book("  source:123  ", "RU", "  Проба  ", KEY).unwrap();
        assert_eq!(book.target_language, "ru");
        assert_eq!(book.title, "Проба");
        assert!(new_book("source:123", "ru", "Проба", "bad").is_err());
    }

    #[test]
    fn chapter_is_published_on_save_with_a_fixed_maximum_length() {
        fn input(body: &str) -> SaveChapterRequest<'_> {
            SaveChapterRequest {
                book_id: 1,
                chapter_id: None,
                number: 1,
                expected_revision: 0,
                title: "Глава",
                body,
                access: "free",
                price_amount: 0,
                command_key: KEY,
            }
        }
        assert_eq!(
            save_chapter(input(&"я".repeat(50_000)))
                .unwrap()
                .body
                .chars()
                .count(),
            50_000
        );
        assert_eq!(
            save_chapter(input(&"я".repeat(50_001))).unwrap_err(),
            AuthorError::Invalid
        );
        assert!(save_chapter(input("")).is_err());
        assert!(save_chapter(SaveChapterRequest {
            expected_revision: 1,
            ..input("hello")
        })
        .is_err());
        assert_eq!(save_chapter(input("a\r\nb")).unwrap().body, "a\nb");
    }

    #[test]
    fn paid_chapter_requires_a_positive_price() {
        let base = SaveChapterRequest {
            book_id: 1,
            chapter_id: Some(1),
            number: 1,
            expected_revision: 1,
            title: "Глава",
            body: "Текст",
            access: "paid",
            price_amount: 0,
            command_key: KEY,
        };
        assert_eq!(save_chapter(base).unwrap_err(), AuthorError::Invalid);
    }
}
