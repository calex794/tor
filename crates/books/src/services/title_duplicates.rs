//! NEwBOO parity: present a small, advisory title shortlist before creation.
//! Identical titles can legitimately belong to different original works.
use crate::services::authoring::{AuthorError, NewBook};
use serde::Serialize;

pub const MAX_SUGGESTIONS: usize = 5;
const MAX_ALTERNATIVES: usize = 3;

#[derive(Clone, Debug, Serialize)]
pub struct TitleCandidate {
    pub id: i64,
    pub title: String,
    pub target_language: String,
    pub exact: bool,
    pub score: f32,
}

/// Input contract shared by form submission and read-only candidate lookup.
/// No unconstrained fuzzy scans and no user-supplied SQL expressions.
pub fn candidate_titles(
    title: &str,
    original_title: &str,
    alternative_titles: &str,
) -> Result<Vec<String>, AuthorError> {
    let alternatives: Vec<&str> = alternative_titles
        .split(|c: char| matches!(c, ',' | ';' | '\n' | '|'))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect();
    if alternatives.len() > MAX_ALTERNATIVES {
        return Err(AuthorError::Invalid);
    }
    let mut unique: Vec<String> = Vec::new();
    for value in std::iter::once(title)
        .chain(std::iter::once(original_title))
        .chain(alternatives)
    {
        let cleaned = value.trim();
        if cleaned.is_empty() {
            continue;
        }
        if cleaned.chars().count() > 200 || cleaned.chars().any(char::is_control) {
            return Err(AuthorError::Invalid);
        }
        if !unique
            .iter()
            .any(|existing| existing.to_lowercase() == cleaned.to_lowercase())
        {
            unique.push(cleaned.to_owned());
        }
    }
    Ok(unique)
}

/// Store the original and at most three alternatives with the new draft.
/// Main title is indexed by a PostgreSQL trigger, not duplicated here.
pub fn with_title_aliases(
    mut book: NewBook,
    original_title: &str,
    alternative_titles: &str,
) -> Result<NewBook, AuthorError> {
    let inputs = candidate_titles(&book.title, original_title, alternative_titles)?;
    let original = original_title.trim();
    if !original.is_empty() && !original.eq_ignore_ascii_case(&book.title) {
        book.original_title = original.to_owned();
    }
    book.alternative_titles = inputs
        .into_iter()
        .filter(|value| {
            !value.eq_ignore_ascii_case(&book.title)
                && !value.eq_ignore_ascii_case(&book.original_title)
        })
        .collect();
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::authoring::new_book;

    #[test]
    fn deduplicates_names_and_limits_alternatives() {
        let names = candidate_titles(
            "Синяя фиалка",
            "Blue Violet",
            "СИНЯЯ ФИАЛКА; Голубая фиалка",
        )
        .unwrap();
        assert_eq!(names, ["Синяя фиалка", "Blue Violet", "Голубая фиалка"]);
        assert_eq!(
            candidate_titles("Название", "", "а,б,в,г"),
            Err(AuthorError::Invalid)
        );
        assert_eq!(
            candidate_titles("Название", "", &"x".repeat(201)),
            Err(AuthorError::Invalid)
        );
    }

    #[test]
    fn aliases_are_optional_and_keep_main_title_unchanged() {
        let book = new_book("source:19", "ru", "Синяя фиалка", &"k".repeat(43)).unwrap();
        let old = with_title_aliases(book, "", "").unwrap();
        assert!(old.original_title.is_empty());
        assert!(old.alternative_titles.is_empty());
        let updated = with_title_aliases(old, "Blue Violet", "Голубая фиалка").unwrap();
        assert_eq!(updated.title, "Синяя фиалка");
        assert_eq!(updated.original_title, "Blue Violet");
        assert_eq!(updated.alternative_titles, vec!["Голубая фиалка"]);
    }
}
