#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchBook {
    pub id: u64,
    pub title: String,
    pub language: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPage {
    pub items: Vec<SearchBook>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchError {
    Invalid,
    Unavailable,
}
pub fn validate(term: &str, after: Option<&str>, limit: usize) -> Result<i64, SearchError> {
    if term.trim().is_empty()
        || term.trim().chars().count() > 80
        || term.chars().any(char::is_control)
        || !(1..=100).contains(&limit)
    {
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

/// Language identifiers use the same bound as the canonical books table.
pub fn normalize_language(raw: Option<&str>) -> Result<Option<String>, SearchError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 32
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
    fn f_03_normalizes_language_and_rejects_malformed_filters() {
        assert_eq!(normalize_language(Some(" RU ")), Ok(Some("ru".to_owned())));
        assert_eq!(normalize_language(Some("en-us")), Ok(Some("en-us".to_owned())));
        assert_eq!(normalize_language(None), Ok(None));
        for value in ["", "ru/../../", "-ru", "ru-", "en us", "a@example"] {
            assert_eq!(normalize_language(Some(value)), Err(SearchError::Invalid));
        }
        assert_eq!(normalize_language(Some(&"x".repeat(33))), Err(SearchError::Invalid));
    }
}
