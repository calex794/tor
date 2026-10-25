use std::{error::Error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BookError {
    InvalidId,
    EmptySourceIdentity,
    InvalidLanguage,
    InvalidTitle,
}

impl fmt::Display for BookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for BookError {}

/// A canonical work is identified by the source work AND translation language.
/// A URL slug or team membership must never be used as this identity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BookKey {
    source_identity: String,
    target_language: String,
}

impl BookKey {
    pub fn new(
        source_identity: impl Into<String>,
        target_language: impl Into<String>,
    ) -> Result<Self, BookError> {
        let source_identity = source_identity.into().trim().to_owned();
        let target_language = target_language.into().trim().to_ascii_lowercase();
        if source_identity.is_empty() {
            return Err(BookError::EmptySourceIdentity);
        }
        if target_language.is_empty()
            || target_language.starts_with('-')
            || target_language.ends_with('-')
            || !target_language
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(BookError::InvalidLanguage);
        }
        Ok(Self {
            source_identity,
            target_language,
        })
    }

    pub fn source_identity(&self) -> &str {
        &self.source_identity
    }

    pub fn target_language(&self) -> &str {
        &self.target_language
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    Draft,
    Public,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Book {
    id: u64,
    key: BookKey,
    title: String,
    visibility: Visibility,
}

impl Book {
    pub fn new(
        id: u64,
        key: BookKey,
        title: impl Into<String>,
        visibility: Visibility,
    ) -> Result<Self, BookError> {
        if id == 0 {
            return Err(BookError::InvalidId);
        }
        let title = title.into().trim().to_owned();
        if title.is_empty() || title.chars().count() > 200 {
            return Err(BookError::InvalidTitle);
        }
        Ok(Self {
            id,
            key,
            title,
            visibility,
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn key(&self) -> &BookKey {
        &self.key
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn visibility(&self) -> Visibility {
        self.visibility
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_is_source_plus_language_not_slug() {
        let a = BookKey::new("source:123", "RU").expect("valid key");
        let b = BookKey::new("source:123", "ru").expect("valid key");
        let c = BookKey::new("source:123", "en").expect("valid key");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.target_language(), "ru");
    }

    #[test]
    fn empty_and_invalid_identifiers_are_rejected() {
        assert_eq!(BookKey::new(" ", "ru"), Err(BookError::EmptySourceIdentity));
        assert_eq!(BookKey::new("x", "../ru"), Err(BookError::InvalidLanguage));
        let key = BookKey::new("source:x", "ru").expect("valid key");
        assert_eq!(
            Book::new(0, key, "Title", Visibility::Public),
            Err(BookError::InvalidId)
        );
    }

    #[test]
    fn invalid_titles_are_rejected() {
        let key = BookKey::new("source:123", "ru").expect("valid key");
        assert_eq!(
            Book::new(1, key, "  ", Visibility::Draft),
            Err(BookError::InvalidTitle)
        );
    }
}
