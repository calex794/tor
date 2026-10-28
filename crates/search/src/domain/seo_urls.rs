//! Stable public book identity. Neither title nor editable slug affects the URL.
//! This module does not decide whether a book is visible or indexable.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BookUrl(u64);

impl BookUrl {
    pub fn from_id(id: u64) -> Option<Self> {
        (id > 0).then_some(Self(id))
    }

    pub fn from_exact_path(path: &str) -> Option<Self> {
        let raw = path.strip_prefix("/books/")?;
        if raw.is_empty() || raw.starts_with('0') || !raw.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let url = Self::from_id(raw.parse::<u64>().ok()?)?;
        (url.path() == path).then_some(url)
    }

    pub fn matches_path(self, path: &str) -> bool {
        Self::from_exact_path(path) == Some(self)
    }

    pub fn id(self) -> u64 {
        self.0
    }

    pub fn path(self) -> String {
        format!("/books/{}", self.0)
    }
}

/// SEO-04 preparation: only a trusted server configuration may supply this
/// origin. Never construct it from Host or X-Forwarded-Host request headers.
/// No HTTP metadata or indexability is activated by this type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedSeoOrigin(String);

impl TrustedSeoOrigin {
    pub fn from_config(value: &str) -> Option<Self> {
        let authority = value.strip_prefix("https://")?;
        let authority = authority.strip_suffix('/').unwrap_or(authority);
        if authority.is_empty()
            || authority.len() > 253
            || !authority.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
            })
        {
            return None;
        }
        let labels: Vec<_> = authority.split('.').collect();
        if labels.len() < 2
            || labels.iter().any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
            })
            || labels.last()?.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        Some(Self(format!("https://{authority}")))
    }

    pub fn book_url(&self, book: BookUrl) -> String {
        format!("{}{}", self.0, book.path())
    }
}
