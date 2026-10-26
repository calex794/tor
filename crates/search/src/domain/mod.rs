#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchBook {
    pub id: u64,
    pub title: String,
    pub language: String,
    pub author_name: String,
    pub genres: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPage {
    pub items: Vec<SearchBook>,
    pub next_cursor: Option<String>,
}
/// Stable keyset directions. These describe book identity order, not popularity
/// or publication timestamp: those are separate roadmap capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogSort {
    Oldest,
    Newest,
    Title,
    Author,
}

impl CatalogSort {
    pub fn parse(raw: Option<&str>) -> Result<Self, SearchError> {
        match raw {
            None | Some("oldest") => Ok(Self::Oldest),
            Some("newest") => Ok(Self::Newest),
            Some("title") => Ok(Self::Title),
            Some("author") => Ok(Self::Author),
            _ => Err(SearchError::Invalid),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oldest => "oldest",
            Self::Newest => "newest",
            Self::Title => "title",
            Self::Author => "author",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchError {
    Invalid,
    Unavailable,
}
fn validate_page(after: Option<&str>, limit: usize) -> Result<i64, SearchError> {
    if !(1..=100).contains(&limit) {
        return Err(SearchError::Invalid);
    }
    let after = match after {
        None => 0,
        Some(value)
            if !value.is_empty()
                && !value.starts_with('0')
                && value.bytes().all(|c| c.is_ascii_digit()) =>
        {
            value.parse::<i64>().map_err(|_| SearchError::Invalid)?
        }
        Some(_) => return Err(SearchError::Invalid),
    };
    if after < 0 {
        return Err(SearchError::Invalid);
    }
    Ok(after)
}

pub fn validate(term: &str, after: Option<&str>, limit: usize) -> Result<i64, SearchError> {
    if term.trim().is_empty()
        || term.trim().chars().count() > 80
        || term.chars().any(char::is_control)
    {
        return Err(SearchError::Invalid);
    }
    validate_page(after, limit)
}

pub fn normalize_genre(raw: Option<&str>) -> Result<Option<String>, SearchError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.chars().any(char::is_control) {
        return Err(SearchError::Invalid);
    }
    let genre = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if genre.is_empty() {
        return Ok(None);
    }
    if genre.chars().count() > 40 || genre.contains(',') {
        return Err(SearchError::Invalid);
    }
    Ok(Some(genre))
}

pub fn validate_facets(
    term: &str,
    language: Option<&str>,
    genre: Option<&str>,
    after: Option<&str>,
    limit: usize,
) -> Result<(i64, Option<String>, Option<String>), SearchError> {
    let normalized = validate_browse(term, language, genre, after, limit)?;
    if term.trim().is_empty() && normalized.1.is_none() && normalized.2.is_none() {
        return Err(SearchError::Invalid);
    }
    Ok(normalized)
}

/// The full public catalog can be paged even when no search filters are set.
/// This is deliberately different from an unqualified legacy search request.
pub fn validate_browse(
    term: &str,
    language: Option<&str>,
    genre: Option<&str>,
    after: Option<&str>,
    limit: usize,
) -> Result<(i64, Option<String>, Option<String>), SearchError> {
    let language = normalize_language(language)?;
    let genre = normalize_genre(genre)?;
    if term.trim().chars().count() > 80 || term.chars().any(char::is_control) {
        return Err(SearchError::Invalid);
    }
    Ok((validate_page(after, limit)?, language, genre))
}

/// Language identifiers use the same bound as the canonical books table.
pub fn normalize_language(raw: Option<&str>) -> Result<Option<String>, SearchError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 32
        || value.starts_with('-')
        || value.ends_with('-')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(SearchError::Invalid);
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn f_03_rejects_bad_queries() {
        assert_eq!(validate(" ", None, 20), Err(SearchError::Invalid));
        assert_eq!(validate("x", None, 101), Err(SearchError::Invalid));
        assert_eq!(validate("x", Some("01"), 20), Err(SearchError::Invalid));
        assert_eq!(validate("x", Some("-2"), 20), Err(SearchError::Invalid));
        assert_eq!(validate("<script>", None, 20), Ok(0));
    }

    #[test]
    fn f_03_author_genre_query_accepts_facets_without_text() {
        assert_eq!(
            normalize_genre(Some("  Фэнтези   и фантастика  ")),
            Ok(Some("фэнтези и фантастика".to_owned()))
        );
        assert_eq!(normalize_genre(Some("  ")), Ok(None));
        assert_eq!(normalize_genre(Some("a,b")), Err(SearchError::Invalid));
        assert_eq!(
            normalize_genre(Some("bad\nnews")),
            Err(SearchError::Invalid)
        );
        assert_eq!(
            validate_facets("", None, None, None, 20),
            Err(SearchError::Invalid)
        );
        assert_eq!(
            validate_facets("", None, Some("Фэнтези"), None, 20)
                .unwrap()
                .0,
            0
        );
        assert_eq!(
            validate_facets("", Some("ru"), None, Some("01"), 20),
            Err(SearchError::Invalid)
        );
    }

    #[test]
    fn f_02_rejects_unknown_order_and_keeps_legacy_query_policy() {
        assert_eq!(CatalogSort::parse(None), Ok(CatalogSort::Oldest));
        assert_eq!(CatalogSort::parse(Some("oldest")), Ok(CatalogSort::Oldest));
        assert_eq!(CatalogSort::parse(Some("newest")), Ok(CatalogSort::Newest));
        assert_eq!(CatalogSort::parse(Some("title")), Ok(CatalogSort::Title));
        assert_eq!(CatalogSort::parse(Some("author")), Ok(CatalogSort::Author));
        for invalid in ["", "latest", "newest;drop", "NEWEST"] {
            assert_eq!(CatalogSort::parse(Some(invalid)), Err(SearchError::Invalid));
        }
        assert_eq!(
            validate_facets("", None, None, None, 20),
            Err(SearchError::Invalid)
        );
        assert_eq!(validate_browse("", None, None, None, 20).unwrap().0, 0);
        assert_eq!(
            validate_browse("", None, None, Some("00"), 20),
            Err(SearchError::Invalid)
        );
        assert_eq!(
            validate_browse("", None, None, None, 101),
            Err(SearchError::Invalid)
        );
    }

    #[test]
    fn f_03_normalizes_language_and_rejects_malformed_filters() {
        assert_eq!(normalize_language(Some(" RU ")), Ok(Some("ru".to_owned())));
        assert_eq!(
            normalize_language(Some("en-us")),
            Ok(Some("en-us".to_owned()))
        );
        assert_eq!(normalize_language(None), Ok(None));
        assert_eq!(normalize_language(Some("")), Ok(None));
        assert_eq!(normalize_language(Some("  ")), Ok(None));
        for value in ["ru/../../", "-ru", "ru-", "en us", "a@example"] {
            assert_eq!(normalize_language(Some(value)), Err(SearchError::Invalid));
        }
        assert_eq!(
            normalize_language(Some(&"x".repeat(33))),
            Err(SearchError::Invalid)
        );
    }
}
