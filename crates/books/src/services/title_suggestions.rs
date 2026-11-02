//! B-02/S019: safe, bounded pre-registration searches; no private-book data.
use crate::services::{authoring::AuthorError, book_registration::normalized_title};

pub const MAX_SUGGESTION_TERMS: usize = 5;
pub const MAX_SUGGESTIONS: i64 = 6;

/// Search terms are entered titles, not guesses at translations.
/// At least three Unicode characters avoids expensive per-keystroke scans.
pub fn normalize_terms(titles: &[&str]) -> Result<Vec<String>, AuthorError> {
    if titles.len() > MAX_SUGGESTION_TERMS {
        return Err(AuthorError::Invalid);
    }
    let mut terms = Vec::new();
    for title in titles {
        if title.chars().count() > 200 || title.chars().any(char::is_control) {
            return Err(AuthorError::Invalid);
        }
        let normalized = normalized_title(title);
        if normalized.chars().count() >= 3 && !terms.contains(&normalized) {
            terms.push(normalized);
        }
    }
    Ok(terms)
}

pub fn target_language(value: &str) -> Result<String, AuthorError> {
    let language = value.trim().to_ascii_lowercase();
    if language.is_empty()
        || language.len() > 32
        || !language
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
    {
        return Err(AuthorError::Invalid);
    }
    Ok(language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s019_preflight_supports_cjk_and_english_without_guessing_translation() {
        let terms = normalize_terms(&[
            " Путь Бессмертного! ",
            "修仙之路",
            "The Path to Immortality",
            "修仙之路",
            "",
        ])
        .unwrap();
        assert_eq!(
            terms,
            ["путь бессмертного", "修仙之路", "the path to immortality"]
        );
        assert_eq!(target_language(" RU ").unwrap(), "ru");
        assert!(normalize_terms(&["ab"]).unwrap().is_empty());
        assert!(normalize_terms(&["\u{0000}title"]).is_err());
        assert!(normalize_terms(&["a".repeat(201).as_str()]).is_err());
        assert!(normalize_terms(&["a", "b", "c", "d", "e", "f"]).is_err());
        assert!(target_language("ru;DROP").is_err());
    }
}
