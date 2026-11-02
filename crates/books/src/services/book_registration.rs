//! S019: canonical source identity and metadata for a manually registered book.
use crate::services::authoring::{valid_command_key, AuthorError, NewBook};
use axum::http::Uri;
use sha2::{Digest, Sha256};
use std::net::IpAddr;

pub struct RegistrationFields<'a> {
    pub source_url: &'a str,
    pub original_title: &'a str,
    pub original_language: &'a str,
    pub release_year: &'a str,
    pub project_status: &'a str,
    pub catalog_section: &'a str,
    pub alt_titles: [&'a str; 3],
}

/// The command key prevents double submission, not duplicate source works.
/// Manual originals without a source URL are additionally guarded by title claims.
pub fn source_identity(
    source_url: &str,
    actor: i64,
    command_key: &str,
) -> Result<String, AuthorError> {
    if actor <= 0 || !valid_command_key(command_key) {
        return Err(AuthorError::Invalid);
    }
    let source_url = source_url.trim();
    if source_url.is_empty() {
        return Ok(format!("manual:{actor}:{command_key}"));
    }
    if source_url.len() > 512
        || source_url.contains('#')
        || source_url
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return Err(AuthorError::Invalid);
    }
    let uri: Uri = source_url.parse().map_err(|_| AuthorError::Invalid)?;
    let scheme = uri.scheme_str().ok_or(AuthorError::Invalid)?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(AuthorError::Invalid);
    }
    let authority = uri.authority().ok_or(AuthorError::Invalid)?;
    let host = authority.host().trim_end_matches('.').to_ascii_lowercase();
    if authority.as_str().contains('@')
        || !host.contains('.')
        || host.ends_with(".local")
        || host.parse::<IpAddr>().is_ok()
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
    {
        return Err(AuthorError::Invalid);
    }
    let port = authority.port_u16();
    if port.is_some_and(|value| value != 80 && value != 443) {
        return Err(AuthorError::Invalid);
    }
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let path = uri.path().trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    let query = uri
        .query()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    // Only a proven work-page ID may ignore the source's optional query.
    // Other sites retain their exact existing URL identity contract.
    if let Some(work_id) = rulate_work_id(source_url) {
        return Ok(format!("rulate:book:{work_id}"));
    }
    // HTTP and HTTPS links to the same public work share one stable identity.
    let canonical = format!("https://{host}{path}{query}");
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(format!("url:{digest:x}"))
}

/// Stable public work number shared by the provider's canonical book links.
/// Never infer identity from fuzzy titles, chapter links or unrelated hosts.
pub fn rulate_work_id(url: &str) -> Option<String> {
    if url.contains('#')
        || url
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return None;
    }
    let uri: Uri = url.parse().ok()?;
    if !matches!(uri.scheme_str()?, "http" | "https") {
        return None;
    }
    let authority = uri.authority()?;
    if authority.as_str().contains('@')
        || authority
            .port_u16()
            .is_some_and(|port| port != 80 && port != 443)
    {
        return None;
    }
    let host = authority.host().to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "rulate.ru" | "tl.rulate.ru" | "www.rulate.ru" | "www.tl.rulate.ru"
    ) {
        return None;
    }
    let path = uri.path().strip_prefix("/book/")?;
    let raw_id = path.strip_suffix('/').unwrap_or(path);
    if raw_id.is_empty() || raw_id.len() > 18 || !raw_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id = raw_id.trim_start_matches('0');
    if id.is_empty() {
        return None;
    }
    Some(id.to_owned())
}

/// Match exact title aliases after case, punctuation, spacing and ё/е cleanup.
pub fn normalized_title(value: &str) -> String {
    let mut normalized = String::new();
    for ch in value.trim().to_lowercase().chars() {
        if ch.is_alphanumeric() {
            normalized.push(if ch == 'ё' { 'е' } else { ch });
        } else if !normalized.is_empty() && !normalized.ends_with(' ') {
            normalized.push(' ');
        }
    }
    normalized.trim_end().to_owned()
}

pub fn registration_titles(book: &NewBook) -> Result<Vec<String>, AuthorError> {
    let mut titles = Vec::new();
    for title in std::iter::once(&book.title)
        .chain(std::iter::once(&book.original_title))
        .chain(book.alt_titles.iter())
    {
        if title.is_empty() {
            continue;
        }
        let normalized = normalized_title(title);
        if normalized.is_empty() {
            return Err(AuthorError::Invalid);
        }
        if !titles.contains(&normalized) {
            titles.push(normalized);
        }
    }
    Ok(titles)
}

/// A user can attach translation/mirror links without treating them as the
/// original work. A URL's identity is exact or provider-specific, never fuzzy.
pub fn with_extra_source_links(
    mut book: NewBook,
    raw_links: [&str; 2],
) -> Result<NewBook, AuthorError> {
    let mut seen = Vec::new();
    if !book.source_url.is_empty() {
        seen.push(source_identity(&book.source_url, 1, &book.command_key)?);
    }
    for raw in raw_links {
        let url = raw.trim();
        if url.is_empty() {
            continue;
        }
        let identity = source_identity(url, 1, &book.command_key)?;
        if seen.contains(&identity) {
            continue;
        }
        seen.push(identity);
        book.extra_source_urls.push(url.to_owned());
    }
    Ok(book)
}

/// Primary and additional source links must make the same atomic claims.
pub fn registration_link_claims(book: &NewBook) -> Result<Vec<(String, String)>, AuthorError> {
    if book.extra_source_urls.len() > 2 {
        return Err(AuthorError::Invalid);
    }
    let mut claims: Vec<(String, String)> = Vec::new();
    for raw in std::iter::once(&book.source_url).chain(book.extra_source_urls.iter()) {
        if raw.is_empty() {
            continue;
        }
        let identity = source_identity(raw, 1, &book.command_key)?;
        if claims.iter().all(|(existing, _)| existing != &identity) {
            claims.push((identity, raw.to_owned()));
        }
    }
    Ok(claims)
}

pub fn with_registration_metadata(
    mut book: NewBook,
    fields: RegistrationFields<'_>,
) -> Result<NewBook, AuthorError> {
    let original_title = fields.original_title.trim();
    let original_language = fields.original_language.trim().to_ascii_lowercase();
    let source_url = fields.source_url.trim();
    let status = fields.project_status.trim();
    let catalog = fields.catalog_section.trim();
    if original_title.chars().count() > 200
        || original_title.chars().any(char::is_control)
        || original_language.len() > 32
        || (!original_language.is_empty()
            && !original_language
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c == b'-'))
        || source_url.len() > 512
        || !matches!(status, "" | "ongoing" | "completed" | "paused" | "planned")
        || (!catalog.is_empty()
            && !matches!(
                catalog,
                "novels/chinese"
                    | "novels/korean"
                    | "novels/japanese"
                    | "novels/english"
                    | "novels/original"
                    | "fanfic/translated"
            ))
    {
        return Err(AuthorError::Invalid);
    }
    let release_year = if fields.release_year.trim().is_empty() {
        None
    } else {
        let year = fields.release_year.trim();
        if year.len() != 4 || !year.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(AuthorError::Invalid);
        }
        let year: i32 = year.parse().map_err(|_| AuthorError::Invalid)?;
        if !(1000..=9999).contains(&year) {
            return Err(AuthorError::Invalid);
        }
        Some(year)
    };
    let mut alt_titles = Vec::new();
    for value in fields.alt_titles {
        let clean = value.trim();
        if clean.is_empty() {
            continue;
        }
        if clean.chars().count() > 200 || clean.chars().any(char::is_control) {
            return Err(AuthorError::Invalid);
        }
        if !alt_titles.iter().any(|previous: &String| previous == clean) {
            alt_titles.push(clean.to_owned());
        }
    }
    book.original_title = original_title.to_owned();
    book.original_language = original_language;
    book.source_url = source_url.to_owned();
    book.release_year = release_year;
    book.project_status = status.to_owned();
    book.catalog_section = catalog.to_owned();
    book.alt_titles = alt_titles;
    registration_titles(&book)?;
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::authoring::new_book;

    const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn b_02_source_identity_uses_work_not_form_token() {
        let a = source_identity("https://www.Example.org/Book/10/", 5, KEY).unwrap();
        let b = source_identity("http://example.org/Book/10", 9, &"b".repeat(43)).unwrap();
        assert_eq!(a, b, "one work, regardless of owner or command key");
        assert_ne!(
            a,
            source_identity("https://example.org/Book/11", 5, KEY).unwrap()
        );
        assert_ne!(
            source_identity("", 5, KEY).unwrap(),
            source_identity("", 6, KEY).unwrap()
        );
        for invalid in [
            "file:///etc/passwd",
            "/book/1",
            "https://127.0.0.1/a",
            "https://user@example.org/a",
            "https://example.org:8080/a",
            "https://example.org/a#fragment",
        ] {
            assert!(source_identity(invalid, 5, KEY).is_err(), "{invalid}");
        }
    }

    #[test]
    fn s019_rulate_url_aliases_share_one_proven_work_id() {
        let variants = [
            "https://tl.rulate.ru/book/268849",
            "http://rulate.ru/book/000268849/?utm_source=share",
            "https://www.tl.rulate.ru/book/268849?ref=example",
        ];
        for (i, link) in variants.iter().enumerate() {
            assert_eq!(rulate_work_id(link).as_deref(), Some("268849"));
            assert_eq!(
                source_identity(link, 10 + i as i64, KEY).unwrap(),
                "rulate:book:268849"
            );
        }
        assert_eq!(
            source_identity("https://tl.rulate.ru/book/268850", 10, KEY).unwrap(),
            "rulate:book:268850"
        );
        for unrelated in [
            "https://notrulate.ru/book/268849",
            "https://tl.rulate.ru/book/268849/chapter/1",
            "https://tl.rulate.ru/book/0",
            "https://tl.rulate.ru/books/268849",
            "https://tl.rulate.ru/book/268849#chapters",
        ] {
            assert!(rulate_work_id(unrelated).is_none(), "{unrelated}");
        }
        assert_ne!(
            source_identity("https://example.org/a?chapter=1", 12, KEY).unwrap(),
            source_identity("https://example.org/a?chapter=2", 12, KEY).unwrap(),
            "other providers retain semantic query parameters"
        );
    }

    #[test]
    fn s019_extra_links_accept_any_safe_host_without_conflating_originals() {
        let book = new_book("manual:7:test", "ru", "Книга", KEY).unwrap();
        let book = with_extra_source_links(
            book,
            [
                "https://example.cn/novel/abc",
                "https://tl.rulate.ru/book/268849",
            ],
        )
        .unwrap();
        let links = registration_link_claims(&book).unwrap();
        assert_eq!(links.len(), 2);
        assert_ne!(links[0].0, links[1].0);
        assert_eq!(book.extra_source_urls.len(), 2);

        let original = "https://example.cn/novel/abc";
        let book = new_book("manual:7:test", "ru", "Книга", KEY).unwrap();
        let book = with_extra_source_links(book, [original, original]).unwrap();
        assert_eq!(registration_link_claims(&book).unwrap().len(), 1);

        let book = new_book("manual:7:test", "ru", "Книга", KEY).unwrap();
        assert!(with_extra_source_links(book, ["javascript:alert(1)", ""]).is_err());
    }

    #[test]
    fn b_02_aliases_normalize_without_duplicate_claims() {
        let book = new_book("manual:5:key", "ru", "Ёж — и кот", KEY).unwrap();
        let book = with_registration_metadata(
            book,
            RegistrationFields {
                source_url: "",
                original_title: "Еж и кот",
                original_language: "zh",
                release_year: "2023",
                project_status: "ongoing",
                catalog_section: "novels/chinese",
                alt_titles: ["ЕЖ, И КОТ", "Другой заголовок", ""],
            },
        )
        .unwrap();
        assert_eq!(registration_titles(&book).unwrap().len(), 2);
        assert_eq!(book.release_year, Some(2023));
        assert_eq!(normalized_title(" Ёж — и   кот "), "еж и кот");
    }

    #[test]
    fn b_02_rejects_invalid_registration_metadata() {
        let book = new_book("manual:5:key", "ru", "Название", KEY).unwrap();
        assert!(with_registration_metadata(
            book,
            RegistrationFields {
                source_url: "",
                original_title: "",
                original_language: "zh",
                release_year: "999",
                project_status: "ongoing",
                catalog_section: "novels/chinese",
                alt_titles: ["", "", ""],
            }
        )
        .is_err());
    }
}
