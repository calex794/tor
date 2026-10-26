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
        Some(value) if !value.is_empty()
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
}
